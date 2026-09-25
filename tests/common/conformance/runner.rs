//! Execution of conformance cases.
//!
//! Each case gets its own fixture. The expectation is resolved once — from the
//! reference shells for POSIX cases, or from the case's literal expectation for
//! native ones — and then fsh is run in every requested engine against the same
//! pristine tree. Every invocation resets the fixture first, so `$HOME` and
//! glob results stay identical across shells while file-mutating cases cannot
//! contaminate one another.

use std::fmt::Write as _;
use std::path::Path;

use super::case::{Case, Engine, Oracle};
use super::compare::{FailureClass, Mismatch, compare};
use super::fixture::{ENGINE_TRACE_FILE, Fixture};
use super::oracle::{ReferenceShell, bash_reference, posix_references};
use super::outcome::{Invocation, Outcome, capture};

/// The routing decision fsh recorded for one invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineTrace {
    pub engine: String,
    pub reason: String,
}

impl EngineTrace {
    /// Parse the *first* trace line, which is the top-level decision.
    ///
    /// Later lines come from an engine re-executing fsh in a subshell (a POSIX
    /// script's child shells do), so the first line is the one that describes
    /// the input under test.
    fn parse(trace: &str) -> Option<Self> {
        let line = trace.lines().find(|line| !line.trim().is_empty())?;
        let mut engine = None;
        let mut reason = String::new();
        for field in line.split_whitespace() {
            if let Some(value) = field.strip_prefix("engine=") {
                engine = Some(value.to_string());
            }
            if let Some(value) = field.strip_prefix("reason=") {
                reason = value.to_string();
            }
        }
        Some(Self {
            engine: engine?,
            reason,
        })
    }
}

/// A disagreement between the engine a case requires and the one that ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingMismatch {
    /// Engine the case requires, by trace name.
    pub expected: &'static str,
    /// Engine that actually ran, or `None` when nothing was recorded.
    pub observed: Option<String>,
    /// Why the router chose it, for the report.
    pub reason: String,
}

/// Result of running one case in one engine.
pub struct EngineResult {
    pub engine: Engine,
    pub expected: Outcome,
    pub actual: Option<Outcome>,
    pub mismatch: Option<Mismatch>,
    /// What the router recorded, when it recorded anything.
    pub trace: Option<EngineTrace>,
    /// Routing disagreement, for cases that require a particular engine.
    pub routing: Option<RoutingMismatch>,
}

/// Result of running one case across all its engines.
pub struct CaseReport {
    pub name: &'static str,
    pub script: &'static str,
    pub features: &'static [&'static str],
    pub oracle_label: String,
    pub references: Vec<&'static str>,
    pub results: Vec<EngineResult>,
    /// Set when the case could not be judged, e.g. no usable reference shell.
    pub skip_reason: Option<String>,
}

/// Run a case and collect per-engine results without asserting anything.
pub fn run_case(case: &Case) -> CaseReport {
    let fixture = match Fixture::new(case.name) {
        Ok(fixture) => fixture,
        Err(error) => {
            return CaseReport {
                name: case.name,
                script: case.script,
                features: case.features,
                oracle_label: String::new(),
                references: Vec::new(),
                results: Vec::new(),
                skip_reason: Some(format!("could not create fixture: {error}")),
            };
        }
    };

    let (expected, oracle_label, references, skip_reason) = match &case.oracle {
        Oracle::Expect { stdout, exit } => (
            Outcome::expected(expand_template(stdout, &fixture), *exit),
            "literal expectation".to_string(),
            Vec::new(),
            None,
        ),
        Oracle::Posix | Oracle::BashPosix | Oracle::Bash => {
            match reference_expectation(case, &fixture) {
                Ok(resolved) => (resolved.outcome, resolved.label, resolved.names, None),
                Err(reason) => (
                    Outcome::expected(String::new(), 0),
                    "reference shells".to_string(),
                    Vec::new(),
                    Some(reason),
                ),
            }
        }
    };

    let mut results = Vec::new();
    if skip_reason.is_none() {
        for engine in case.engines {
            let (actual, mismatch, trace) = match run_fsh(case, &fixture, *engine) {
                Ok((outcome, trace)) => {
                    let mismatch = compare(&expected, &outcome, case.compare_stderr);
                    (Some(outcome), mismatch, trace)
                }
                Err(error) => (
                    None,
                    Some(Mismatch {
                        class: FailureClass::Crash,
                        detail: format!("could not run fsh: {error}"),
                    }),
                    None,
                ),
            };
            let routing = routing_mismatch(case, *engine, &trace);
            results.push(EngineResult {
                engine: *engine,
                expected: expected.clone(),
                actual,
                mismatch,
                trace,
                routing,
            });
        }
    }

    CaseReport {
        name: case.name,
        script: case.script,
        features: case.features,
        oracle_label,
        references,
        results,
        skip_reason,
    }
}

/// Run a case and assert its outcome.
///
/// Engines marked as known failures must still mismatch; every other engine
/// must agree with the expectation. Either kind of surprise fails the test, so
/// the corpus cannot silently drift out of date in either direction.
pub fn assert_case(case: &Case) {
    let report = run_case(case);

    if let Some(reason) = &report.skip_reason {
        eprintln!("conformance SKIP {}: {reason}", case.name);
        return;
    }

    let verdict = classify(case, &report);
    if verdict.is_clean() {
        return;
    }

    panic!("{}", render(case, &report, &verdict));
}

/// Run every case in `cases`, then panic once with a combined report.
///
/// A single run surfaces *every* mismatch rather than stopping at the first,
/// which is what makes the corpus useful as a discovery loop. Known failures
/// and skips are summarised on stderr so a green run still reports how much of
/// the corpus is knowingly red.
pub fn assert_suite(cases: &[Case]) {
    let mut report = String::new();
    let mut unexpected = 0usize;
    let mut known_failures = 0usize;
    let mut known_routing_failures = 0usize;
    let mut skipped = 0usize;
    let mut judged = 0usize;

    for case in cases {
        let case_report = run_case(case);

        if let Some(reason) = &case_report.skip_reason {
            skipped += 1;
            eprintln!("conformance SKIP {}: {reason}", case.name);
            continue;
        }
        judged += 1;

        let verdict = classify(case, &case_report);
        let mismatching = case_report
            .results
            .iter()
            .filter(|result| result.mismatch.is_some())
            .count();
        known_failures += mismatching.saturating_sub(verdict.semantic_failures.len());
        known_routing_failures += verdict.routing_known;

        if verdict.is_clean() {
            continue;
        }

        unexpected += verdict.unexpected();
        report.push_str(&render(case, &case_report, &verdict));
        report.push('\n');
    }

    eprintln!(
        "conformance: {} cases, {known_failures} known failures, \
         {known_routing_failures} known routing failures, {skipped} skipped, \
         {unexpected} unexpected",
        cases.len()
    );

    // A suite that judged nothing is not a passing suite: that is the shape of
    // a broken fixture or an unusable oracle, and it must not read as success.
    if judged == 0 && !cases.is_empty() {
        panic!(
            "conformance: no case in this suite could be judged ({skipped} skipped); \
             see the SKIP lines above"
        );
    }

    if !report.is_empty() {
        panic!("{report}");
    }
}

/// How one case's results were judged, split by dimension.
///
/// Semantics and routing are separate verdicts on purpose: "correct output,
/// wrong engine" is a different defect from "wrong output", and folding them
/// into one pass/fail would hide exactly the drift the routing assertions exist
/// to catch.
#[derive(Default)]
struct Verdict<'a> {
    /// Unexpected semantic mismatches.
    semantic_failures: Vec<&'a EngineResult>,
    /// Marked failures that started passing.
    semantic_passes: Vec<&'a EngineResult>,
    /// Unexpected routing disagreements.
    routing_failures: Vec<&'a EngineResult>,
    /// Marked routing failures that started matching.
    routing_passes: Vec<&'a EngineResult>,
    /// Routing disagreements that are marked, so expected.
    routing_known: usize,
}

impl Verdict<'_> {
    /// Nothing to report: every failure is marked and every pass is clean.
    fn is_clean(&self) -> bool {
        self.semantic_failures.is_empty()
            && self.semantic_passes.is_empty()
            && self.routing_failures.is_empty()
            && self.routing_passes.is_empty()
    }

    /// How many problems the suite should count against itself.
    fn unexpected(&self) -> usize {
        self.semantic_failures.len()
            + self.semantic_passes.len()
            + self.routing_failures.len()
            + self.routing_passes.len()
    }
}

/// Split a report's results into the things that should fail the suite.
///
/// Both dimensions are strict: a marked failure that passes is a failure, in
/// either dimension, so a fix cannot land while the corpus still claims the
/// defect is present.
fn classify<'a>(case: &Case, report: &'a CaseReport) -> Verdict<'a> {
    let mut verdict = Verdict::default();

    for result in &report.results {
        let marked = case
            .known_failure
            .as_ref()
            .is_some_and(|known| known.engines.contains(&result.engine));

        match (&result.mismatch, marked) {
            (None, true) => verdict.semantic_passes.push(result),
            (Some(_), false) => verdict.semantic_failures.push(result),
            (None, false) | (Some(_), true) => {}
        }

        match (&result.routing, case.known_routing_failure) {
            (Some(_), Some(_)) => verdict.routing_known += 1,
            (Some(_), None) => verdict.routing_failures.push(result),
            (None, Some(_)) => {
                // Only a run that *had* an expectation can satisfy it.
                if routing_applies(case, result.engine) {
                    verdict.routing_passes.push(result);
                }
            }
            (None, None) => {}
        }
    }

    verdict
}

struct ResolvedExpectation {
    outcome: Outcome,
    label: String,
    names: Vec<&'static str>,
}

/// Establish the expectation by running the installed POSIX reference shells.
///
/// With two or more references available they must agree on stdout and
/// termination; if they do not, the case is a reference disagreement and fsh is
/// not judged at all, because there is no single correct answer to compare to.
/// `Oracle::BashPosix` / `Oracle::Bash` deliberately use a single reference and
/// skip that check.
fn reference_expectation(case: &Case, fixture: &Fixture) -> Result<ResolvedExpectation, String> {
    let references: Vec<ReferenceShell> = match case.oracle {
        Oracle::BashPosix => posix_references()
            .iter()
            .filter(|reference| reference.name.contains("bash"))
            .cloned()
            .collect(),
        Oracle::Bash => bash_reference().into_iter().collect(),
        _ => posix_references().to_vec(),
    };

    if references.is_empty() {
        return Err(match case.oracle {
            Oracle::BashPosix | Oracle::Bash => {
                "no bash installed to serve as the bash oracle".to_string()
            }
            _ => "no POSIX reference shell installed (need bash or dash)".to_string(),
        });
    }

    let mut observed = Vec::new();
    for reference in &references {
        let outcome = run_shell(
            case,
            fixture,
            &reference.program,
            reference.command_args(case.script),
        )
        .map_err(|error| format!("could not run {}: {error}", reference.name))?;
        observed.push((reference.name, outcome));
    }

    if observed.len() > 1 {
        let (primary_name, primary) = &observed[0];
        for (other_name, other) in &observed[1..] {
            if primary.termination != other.termination || primary.stdout != other.stdout {
                return Err(format!(
                    "reference disagreement: {primary_name} ({}) vs {other_name} ({})",
                    primary.summary(),
                    other.summary()
                ));
            }
        }
    }

    let names: Vec<&'static str> = observed.iter().map(|(name, _)| *name).collect();
    let label = names.join(" + ");
    let (_, outcome) = observed.remove(0);

    Ok(ResolvedExpectation {
        outcome,
        label,
        names,
    })
}

fn run_shell(
    case: &Case,
    fixture: &Fixture,
    program: &Path,
    args: Vec<String>,
) -> std::io::Result<Outcome> {
    fixture.reset()?;
    let invocation = Invocation::new(program, fixture.root())
        .args(args)
        .env(fixture.base_env())
        .capture_files(case.files.iter().copied());
    capture(&invocation)
}

fn run_fsh(
    case: &Case,
    fixture: &Fixture,
    engine: Engine,
) -> std::io::Result<(Outcome, Option<EngineTrace>)> {
    let mut args: Vec<String> = engine.flags().iter().map(|flag| flag.to_string()).collect();
    // `--no-color`/`--no-dym` keep diagnostics stable; NO_COLOR is also set in
    // the fixture environment.
    args.push("--no-color".to_string());
    args.push("--no-dym".to_string());
    args.push("-c".to_string());
    args.push(case.script.to_string());
    let outcome = run_shell(case, fixture, fixture.fsh_binary(), args)?;
    let trace = fixture
        .read_file(ENGINE_TRACE_FILE)
        .as_deref()
        .and_then(EngineTrace::parse);
    Ok((outcome, trace))
}

/// Whether a result carries a routing verdict at all.
///
/// Routing is a runtime decision only in auto mode; `--native` and `--posix`
/// choose up front, so they have no routing expectation to meet or miss and
/// must not be counted as routing passes.
fn routing_applies(case: &Case, engine: Engine) -> bool {
    engine == Engine::Auto && case.expected_engine.is_some()
}

/// Whether the engine that ran is the engine the case requires.
///
/// Only auto mode is judged: `--native` and `--posix` decide in advance, so an
/// assertion there would merely restate the flag. A missing trace counts as a
/// disagreement — a case that requires a dispatch must observe one, or a future
/// change could stop recording decisions without the suite noticing.
fn routing_mismatch(
    case: &Case,
    engine: Engine,
    trace: &Option<EngineTrace>,
) -> Option<RoutingMismatch> {
    if !routing_applies(case, engine) {
        return None;
    }
    let expected = case.expected_engine?;
    let observed = trace.as_ref().map(|trace| trace.engine.clone());
    if observed.as_deref() == Some(expected.trace_name()) {
        return None;
    }
    Some(RoutingMismatch {
        expected: expected.trace_name(),
        observed,
        reason: trace
            .as_ref()
            .map(|trace| trace.reason.clone())
            .unwrap_or_else(|| "no routing decision was recorded".to_string()),
    })
}

fn expand_template(text: &str, fixture: &Fixture) -> String {
    text.replace("{HOME}", &fixture.home().to_string_lossy())
        .replace("{ROOT}", &fixture.root().to_string_lossy())
}

/// Render a failure with everything needed to reproduce it, per the spec's
/// requirement that a generated or discovered failure prints its own repro.
fn render(case: &Case, report: &CaseReport, verdict: &Verdict<'_>) -> String {
    let mut out = String::new();
    out.push_str("\n=== conformance mismatch ===\n");
    let _ = writeln!(out, "case      : {}", case.name);
    if !report.features.is_empty() {
        let _ = writeln!(out, "features  : {}", report.features.join(", "));
    }
    let _ = writeln!(out, "oracle    : {}", report.oracle_label);
    if !report.references.is_empty() {
        let _ = writeln!(out, "references: {}", report.references.join(", "));
    }
    let _ = writeln!(
        out,
        "script    :\n    {}",
        case.script.replace('\n', "\n    ")
    );
    if let Some(expected) = case.expected_engine {
        let _ = writeln!(
            out,
            "routing   : auto mode must run this in {}",
            expected.trace_name()
        );
    }
    if let Some(known) = &case.known_failure {
        let _ = writeln!(out, "known     : {} — {}", known.id, known.reason);
    }
    if let Some(reason) = case.known_routing_failure {
        let _ = writeln!(out, "known     : routing — {reason}");
    }

    if !verdict.semantic_failures.is_empty() {
        out.push_str("\nunexpected mismatches:\n");
        for result in &verdict.semantic_failures {
            render_result(&mut out, result);
        }
    }

    if !verdict.semantic_passes.is_empty() {
        out.push_str("\nunexpected passes — this case no longer reproduces the bug:\n");
        for result in &verdict.semantic_passes {
            let _ = writeln!(
                out,
                "  - {} now agrees with {}; remove it from the known_failure list",
                result.engine.label(),
                report.oracle_label
            );
        }
    }

    if !verdict.routing_failures.is_empty() {
        out.push_str("\nunexpected routing:\n");
        for result in &verdict.routing_failures {
            let _ = writeln!(out, "  - {}", result.engine.label());
            match &result.routing {
                Some(routing) => {
                    let observed = routing.observed.as_deref().unwrap_or("<none>");
                    let _ = writeln!(
                        out,
                        "      expected: {} engine, ran {observed} (reason={})",
                        routing.expected, routing.reason
                    );
                }
                None => {
                    let _ = writeln!(out, "      expected: a routing decision, saw none");
                }
            }
            render_trace(&mut out, result);
        }
    }

    if !verdict.routing_passes.is_empty() {
        out.push_str("\nrouting now matches — remove the known_routing_failure marker:\n");
        for result in &verdict.routing_passes {
            let _ = writeln!(out, "  - {}", result.engine.label());
            render_trace(&mut out, result);
        }
    }

    out.push_str("\nreproduce:\n");
    for result in &verdict.semantic_failures {
        let flags = result.engine.flags().join(" ");
        let separator = if flags.is_empty() { "" } else { " " };
        let _ = writeln!(
            out,
            "    fsh{separator}{flags} -c {}",
            shell_quote(case.script)
        );
    }
    if !verdict.routing_failures.is_empty() || !verdict.routing_passes.is_empty() {
        let _ = writeln!(
            out,
            "    FSH_ENGINE_TRACE=/tmp/trace fsh -c {}   # then read /tmp/trace",
            shell_quote(case.script)
        );
    }
    if !report.references.is_empty() {
        let _ = writeln!(
            out,
            "    reference: {} -c {}",
            report.references[0],
            shell_quote(case.script)
        );
    }

    out
}

/// Render what the router recorded, when it recorded anything.
fn render_trace(out: &mut String, result: &EngineResult) {
    match &result.trace {
        Some(trace) => {
            let _ = writeln!(
                out,
                "      trace   : engine={} reason={}",
                trace.engine, trace.reason
            );
        }
        None => {
            let _ = writeln!(out, "      trace   : <none written>");
        }
    }
}

fn render_result(out: &mut String, result: &EngineResult) {
    let class = result
        .mismatch
        .as_ref()
        .map(|mismatch| mismatch.class.label())
        .unwrap_or("?");
    let _ = writeln!(out, "  - {} [{}]", result.engine.label(), class);
    let _ = writeln!(out, "      expected: {}", result.expected.summary());
    match &result.actual {
        Some(actual) => {
            let _ = writeln!(out, "      actual  : {}", actual.summary());
        }
        None => {
            let _ = writeln!(out, "      actual  : <not run>");
        }
    }
    if let Some(mismatch) = &result.mismatch {
        let _ = writeln!(out, "      detail  : {}", mismatch.detail);
    }
}

/// Single-quote a script for pasting into a shell.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
