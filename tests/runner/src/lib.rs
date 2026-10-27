// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Orchestration data and guards shared by the runner and its fault probe.
//!
//! Selection is explicit. Suites name their packages and test targets, curated
//! crate-local replacement families live here (not in nextest profiles), Cargo
//! metadata must classify every specification target and keep the `full`
//! feature lists in parity, and a machine-readable listing must show a live
//! test for every curated family before anything executes. A renamed or
//! deleted family therefore fails the run instead of silently dropping
//! coverage.

use clap::{Parser, ValueEnum};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Suite {
    /// Portable support, library and executable checks.
    All,
    Support,
    Library,
    Executable,
    /// Linux kernel containment probes; requires user/PID namespaces and pidfds.
    Containment,
}

impl Suite {
    pub fn needs_shell(self) -> bool {
        matches!(self, Self::All | Self::Executable)
    }

    pub fn profile(self) -> &'static str {
        match self {
            Self::All => "specifications",
            Self::Support => "support",
            Self::Library => "library",
            Self::Executable => "executable",
            Self::Containment => "containment",
        }
    }

    fn packages(self) -> &'static [&'static str] {
        match self {
            Self::All => &[
                "fshell-test-support",
                "fshell-specifications",
                "fshell-test-runner",
                "fshell-posix",
                "fshell-core",
                "fshell-engine",
            ],
            Self::Support => &["fshell-test-support", "fshell-test-runner"],
            Self::Library => &[
                "fshell-specifications",
                "fshell-posix",
                "fshell-core",
                "fshell-engine",
            ],
            Self::Executable => &["fshell-specifications"],
            Self::Containment => &["fshell-test-support"],
        }
    }

    fn select_targets(self, args: &mut Vec<OsString>) {
        match self {
            // Include runner unit tests; examples and benchmarks are not selected.
            Self::All | Self::Support => {
                args.extend(["--lib", "--bins", "--tests"].map(OsString::from));
            }
            Self::Containment => {
                args.extend(["--test", "namespace_containment"].map(OsString::from));
            }
            Self::Library | Self::Executable => {
                if self == Self::Library {
                    args.push("--lib".into());
                }
                for (target, suite) in SPECIFICATION_TARGETS {
                    if *suite == self {
                        args.extend(["--test", target].map(OsString::from));
                    }
                }
            }
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Run fshell specifications with explicit artifacts and per-test isolation")]
pub struct Arguments {
    #[arg(long, value_enum, default_value = "all")]
    pub suite: Suite,
    /// Build and test all optional shell features.
    #[arg(long)]
    pub full: bool,
    /// Build the selected artifacts and specifications in release mode.
    #[arg(long)]
    pub release: bool,
    /// Explicit native Rust target. Cross execution is not supported here.
    #[arg(long)]
    pub target: Option<String>,
    /// Nextest filter expression. An empty selection fails.
    #[arg(long)]
    pub filter: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subcommand {
    Run,
    List,
}

impl Subcommand {
    fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::List => "list",
        }
    }
}

// Explicit classification: additions/removals must update the execution plan.
// Cargo metadata is checked before builds, so an unclassified target fails.
pub const SPECIFICATION_TARGETS: &[(&str, Suite)] = &[
    ("builtin_registry", Suite::Library),
    ("invocation_state", Suite::Library),
    ("native_evaluation", Suite::Library),
    ("evaluation_effects", Suite::Library),
    ("native_parsing", Suite::Library),
    ("pipeline_values", Suite::Library),
    ("pipeline_execution", Suite::Library),
    ("posix_expansion", Suite::Library),
    ("posix_evaluation", Suite::Library),
    ("posix_io_control", Suite::Library),
    ("search_streams", Suite::Library),
    ("bridge_execution", Suite::Executable),
    ("cli_execution", Suite::Executable),
];

/// Crate-local replacement families exercised by the portable library slice.
/// Each fragment is a nextest `test()` substring filter and must match at least
/// one listed, non-ignored test; [`validate_library_listing`] enforces that.
const POSIX_LIBRARY_FAMILIES: &[&str] = &[
    "posix_builtins::read_cmd::tests::",
    "eval::tests::builtin_output_",
    "expand::field_boundaries::",
    "expand::substitution_bytes::",
    "bridge::subshell_bindings::",
    "positional::ownership::",
    "eval::argument_iteration::",
    "posix_builtins::shift::argument_updates::",
    "eval::capture_transport::",
    "eval::redirection_ownership::",
    "eval::exit_trap_status::",
];
const CORE_LIBRARY_FAMILIES: &[&str] = &["parser::stmt::command_locations::"];
const ENGINE_LIBRARY_FAMILIES: &[&str] = &[
    "pipeline_completion::",
    "pipeline::expression_diagnostics::",
];

/// Package-to-family manifest consumed by both the runner guard and the
/// orchestration fault probe.
pub fn guarded_library_families() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        ("fshell-posix", POSIX_LIBRARY_FAMILIES),
        ("fshell-core", CORE_LIBRARY_FAMILIES),
        ("fshell-engine", ENGINE_LIBRARY_FAMILIES),
    ]
}

fn curated_filter_expression() -> String {
    guarded_library_families()
        .iter()
        .map(|(package, families)| {
            let families = families
                .iter()
                .map(|family| format!("test({family})"))
                .collect::<Vec<_>>()
                .join(" | ");
            format!("(package(={package}) & ({families}))")
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

pub fn library_selection_filter() -> String {
    format!(
        "package(=fshell-specifications) | {}",
        curated_filter_expression()
    )
}

pub fn all_selection_filter() -> String {
    format!(
        "(package(=fshell-test-support) & !binary(=namespace_containment)) | \
         package(=fshell-test-runner) | {}",
        library_selection_filter()
    )
}

pub fn build_arguments(arguments: &Arguments, target: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "build",
        "--locked",
        "--package",
        "fshell",
        "--bin",
        "fsh",
        "--message-format=json-render-diagnostics",
        "--target",
        target,
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    if arguments.release {
        args.push("--release".into());
    }
    if arguments.full {
        args.extend(["--features".into(), "full".into()]);
    }
    args
}

pub fn nextest_arguments(
    arguments: &Arguments,
    target: &str,
    root: &Path,
    subcommand: Subcommand,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["nextest", subcommand.as_str(), "--locked", "--config-file"]
        .into_iter()
        .map(Into::into)
        .collect();
    args.push(root.join("tests/nextest.toml").into_os_string());
    args.extend(["--target", target].map(OsString::from));
    for package in arguments.suite.packages() {
        args.extend(["--package", package].map(OsString::from));
    }
    arguments.suite.select_targets(&mut args);
    if arguments.release {
        args.push("--release".into());
    }
    if arguments.full {
        args.extend(["--features".into(), "fshell-specifications/full".into()]);
    }
    match subcommand {
        Subcommand::Run => {
            args.extend(["--profile", arguments.suite.profile()].map(OsString::from));
            args.extend(["--no-tests", "fail"].map(OsString::from));
            if let Some(filter) = &arguments.filter {
                args.extend([OsString::from("--filter-expr"), filter.into()]);
            } else if matches!(arguments.suite, Suite::All | Suite::Library) {
                let filter = if arguments.suite == Suite::All {
                    all_selection_filter()
                } else {
                    library_selection_filter()
                };
                args.extend([OsString::from("--filter-expr"), filter.into()]);
            }
        }
        Subcommand::List => {
            args.extend(["--message-format", "json"].map(OsString::from));
        }
    }
    args
}

/// Families and listed tests confirmed by [`validate_library_listing`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LibrarySelection {
    pub families: usize,
    pub tests: usize,
}

impl fmt::Display for LibrarySelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} curated families cover {} listed tests",
            self.families, self.tests
        )
    }
}

#[derive(Deserialize)]
struct TestListSummary {
    #[serde(rename = "rust-suites", default)]
    rust_suites: BTreeMap<String, RustSuite>,
}

#[derive(Deserialize)]
struct RustSuite {
    #[serde(rename = "package-name", default)]
    package_name: String,
    #[serde(default)]
    testcases: BTreeMap<String, TestCase>,
}

#[derive(Deserialize)]
struct TestCase {
    #[serde(default)]
    ignored: bool,
}

/// Verify a `cargo nextest list --message-format json` listing advertises at
/// least one non-ignored test for every curated family. Names are matched with
/// the same substring semantics as the generated `test()` filters.
pub fn validate_library_listing(bytes: &[u8]) -> io::Result<LibrarySelection> {
    let summary: TestListSummary = serde_json::from_slice(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut gaps = Vec::new();
    let mut families = 0usize;
    let mut tests = 0usize;
    for (package, package_families) in guarded_library_families() {
        for family in *package_families {
            families += 1;
            let mut listed = 0usize;
            let mut live = 0usize;
            for suite in summary
                .rust_suites
                .values()
                .filter(|suite| suite.package_name == *package)
            {
                for (name, case) in &suite.testcases {
                    if name.contains(family) {
                        listed += 1;
                        live += usize::from(!case.ignored);
                    }
                }
            }
            if live == 0 {
                gaps.push(format!(
                    "{package}::{family} ({})",
                    if listed == 0 {
                        "no listed test"
                    } else {
                        "every match is ignored"
                    }
                ));
            }
            tests += live;
        }
    }
    if !gaps.is_empty() {
        return Err(io::Error::other(format!(
            "library selection guard lost curated families: {}",
            gaps.join(", ")
        )));
    }
    Ok(LibrarySelection { families, tests })
}

#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<MetadataPackage>,
}

#[derive(Deserialize)]
struct MetadataPackage {
    name: String,
    #[serde(default)]
    targets: Vec<CargoTarget>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
}

/// Validate the specification target inventory and the feature parity between
/// the root package and the specification package in one metadata pass.
pub fn validate_package_metadata(output: &[u8]) -> io::Result<()> {
    let metadata: CargoMetadata = serde_json::from_slice(output)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    validate_specification_targets(&metadata)?;
    validate_feature_parity(&metadata)
}

fn unique_package<'a>(metadata: &'a CargoMetadata, name: &str) -> io::Result<&'a MetadataPackage> {
    let packages: Vec<_> = metadata
        .packages
        .iter()
        .filter(|package| package.name == name)
        .collect();
    if packages.len() != 1 {
        return Err(io::Error::other(format!(
            "metadata must contain exactly one {name} package, found {}",
            packages.len()
        )));
    }
    Ok(packages[0])
}

fn validate_specification_targets(metadata: &CargoMetadata) -> io::Result<()> {
    let package = unique_package(metadata, "fshell-specifications")?;
    let mut actual: Vec<_> = package
        .targets
        .iter()
        .filter(|target| target.kind == ["test"])
        .map(|target| target.name.as_str())
        .collect();
    let mut expected: Vec<_> = SPECIFICATION_TARGETS
        .iter()
        .map(|(name, _)| *name)
        .collect();
    actual.sort_unstable();
    expected.sort_unstable();
    if actual != expected {
        return Err(io::Error::other(format!(
            "specification targets must be classified: expected {expected:?}, found {actual:?}"
        )));
    }
    Ok(())
}

/// Every plain feature enabled by the root `full` selection must also be a
/// feature of the specification package with the identical builtin forwarding.
/// Without this, `--full` could silently test a smaller feature set.
fn validate_feature_parity(metadata: &CargoMetadata) -> io::Result<()> {
    let root = unique_package(metadata, "fshell")?;
    let specifications = unique_package(metadata, "fshell-specifications")?;
    let root_full = root
        .features
        .get("full")
        .ok_or_else(|| io::Error::other("fshell must define a full feature"))?;
    let specification_full = specifications
        .features
        .get("full")
        .ok_or_else(|| io::Error::other("fshell-specifications must define a full feature"))?;
    for feature in root_full {
        // `dep:` and crate-relative forwards are wiring, not feature names.
        if feature.contains('/') || feature.contains(':') {
            continue;
        }
        if !specification_full.contains(feature) {
            return Err(io::Error::other(format!(
                "feature parity: fshell full enables {feature}, fshell-specifications full does not"
            )));
        }
        let forwards = |package: &MetadataPackage| -> Vec<String> {
            package
                .features
                .get(feature)
                .into_iter()
                .flatten()
                .filter(|value| value.starts_with("fshell-builtins/"))
                .cloned()
                .collect()
        };
        let root_forwards = forwards(root);
        let specification_forwards = forwards(specifications);
        if root_forwards != specification_forwards {
            return Err(io::Error::other(format!(
                "feature parity: {feature} forwards {root_forwards:?} in fshell versus {specification_forwards:?} in fshell-specifications"
            )));
        }
    }
    Ok(())
}

pub fn native_target(output: &[u8]) -> io::Result<String> {
    let text = std::str::from_utf8(output)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let hosts: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("host: "))
        .collect();
    if hosts.len() != 1 || hosts[0].is_empty() {
        return Err(io::Error::other(
            "compiler output must identify exactly one native target",
        ));
    }
    Ok(hosts[0].into())
}

pub fn validate_target(requested: Option<&str>, host: &str) -> io::Result<String> {
    match requested {
        Some(target) if target != host => Err(io::Error::other(format!(
            "target {target} is not native ({host}); cross execution requires a separate adapter"
        ))),
        Some(target) => Ok(target.into()),
        None => Ok(host.into()),
    }
}

#[derive(Deserialize)]
struct CargoMessage {
    reason: String,
    target: Option<CargoTarget>,
    executable: Option<PathBuf>,
}

pub fn shell_artifact(output: &[u8]) -> io::Result<PathBuf> {
    let mut artifacts = Vec::new();
    for line in output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let message: CargoMessage = serde_json::from_slice(line)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if message.reason == "compiler-artifact"
            && message
                .target
                .is_some_and(|target| target.name == "fsh" && target.kind == ["bin"])
            && let Some(path) = message.executable
        {
            if !path.is_absolute() {
                return Err(io::Error::other("Cargo executable path must be absolute"));
            }
            artifacts.push(path);
        }
    }
    if artifacts.len() != 1 {
        return Err(io::Error::other(format!(
            "expected exactly one fsh executable artifact, found {}",
            artifacts.len()
        )));
    }
    artifacts
        .pop()
        .ok_or_else(|| io::Error::other("missing fsh executable artifact"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments() -> Arguments {
        Arguments::parse_from(["runner"])
    }

    fn has_pair(args: &[OsString], option: &str, value: &str) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == option && pair[1] == value)
    }

    fn has_flag(args: &[OsString], flag: &str) -> bool {
        args.iter().any(|arg| arg == flag)
    }

    #[test]
    fn native_default_overrides_ambient_cargo_target_selection() {
        assert_eq!(
            validate_target(None, "aarch64-unknown-linux-gnu").expect("native target"),
            "aarch64-unknown-linux-gnu"
        );
        assert!(validate_target(Some("x86_64-apple-darwin"), "aarch64-unknown-linux-gnu").is_err());
    }

    #[test]
    fn compiler_host_must_be_unambiguous_and_utf8() {
        assert_eq!(
            native_target(b"release: 1.88.0\nhost: aarch64-apple-darwin\n").expect("host"),
            "aarch64-apple-darwin"
        );
        assert!(native_target(b"release: 1.88.0\n").is_err());
        assert!(native_target(b"host: one\nhost: two\n").is_err());
        assert!(native_target(&[255]).is_err());
    }

    #[test]
    fn profile_target_and_full_features_are_forwarded_to_both_builds() {
        let arguments = Arguments::parse_from(["runner", "--release", "--full"]);
        let build = build_arguments(&arguments, "aarch64-unknown-linux-gnu");
        let run = nextest_arguments(
            &arguments,
            "aarch64-unknown-linux-gnu",
            Path::new("/workspace"),
            Subcommand::Run,
        );
        for args in [&build, &run] {
            assert!(args.contains(&OsString::from("--locked")));
            assert!(args.contains(&OsString::from("--release")));
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--target", "aarch64-unknown-linux-gnu"])
            );
        }
        assert!(build.windows(2).any(|pair| pair == ["--features", "full"]));
        assert!(
            run.windows(2)
                .any(|pair| pair == ["--features", "fshell-specifications/full"])
        );
    }

    #[test]
    fn support_selection_builds_only_support_and_runner_tests() {
        let arguments = Arguments::parse_from(["runner", "--suite", "support"]);
        let args = nextest_arguments(
            &arguments,
            "aarch64-unknown-linux-gnu",
            Path::new("/workspace"),
            Subcommand::Run,
        );
        assert!(args.contains(&OsString::from("fshell-test-support")));
        assert!(args.contains(&OsString::from("fshell-test-runner")));
        assert!(!args.contains(&OsString::from("fshell-specifications")));
        assert!(args.windows(2).any(|pair| pair == ["--no-tests", "fail"]));
        assert!(!has_flag(&args, "--filter-expr"));
    }

    #[test]
    fn containment_selection_prepares_only_its_explicit_support_target() {
        let arguments = Arguments::parse_from(["runner", "--suite", "containment"]);
        assert!(!arguments.suite.needs_shell());
        let args = nextest_arguments(
            &arguments,
            "aarch64-unknown-linux-gnu",
            Path::new("/workspace"),
            Subcommand::Run,
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--test", "namespace_containment"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--profile", "containment"])
        );
        assert!(!args.contains(&OsString::from("fshell-specifications")));
        assert!(!args.contains(&OsString::from("--tests")));
    }

    #[test]
    fn unknown_options_and_suites_are_rejected() {
        assert!(Arguments::try_parse_from(["runner", "--unknown"]).is_err());
        assert!(Arguments::try_parse_from(["runner", "--suite", "typo"]).is_err());
    }

    #[test]
    fn filters_are_forwarded_as_a_single_expression() {
        let mut arguments = arguments();
        arguments.filter = Some("test(arithmetic) | test(invocation)".into());
        let args = nextest_arguments(
            &arguments,
            "aarch64-unknown-linux-gnu",
            Path::new("/workspace"),
            Subcommand::Run,
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--filter-expr", "test(arithmetic) | test(invocation)"])
        );
    }

    #[test]
    fn library_filter_covers_every_guarded_family() {
        for (suite, filter) in [
            (Suite::Library, library_selection_filter()),
            (Suite::All, all_selection_filter()),
        ] {
            assert!(
                filter.contains("package(=fshell-specifications)"),
                "{suite:?}"
            );
            for (package, families) in guarded_library_families() {
                assert!(
                    filter.contains(&format!("package(={package})")),
                    "{suite:?}"
                );
                for family in *families {
                    assert!(filter.contains(&format!("test({family})")), "{family}");
                }
            }
        }
        assert!(all_selection_filter().contains("!binary(=namespace_containment)"));
    }

    #[test]
    fn run_selection_uses_the_suite_filter_and_list_uses_json() {
        for (name, suite) in [("all", Suite::All), ("library", Suite::Library)] {
            let arguments = Arguments::parse_from(["runner", "--suite", name]);
            let run = nextest_arguments(
                &arguments,
                "aarch64-unknown-linux-gnu",
                Path::new("/workspace"),
                Subcommand::Run,
            );
            let filter = if suite == Suite::All {
                all_selection_filter()
            } else {
                library_selection_filter()
            };
            assert!(has_pair(&run, "--filter-expr", &filter), "{suite:?}");
            let list = nextest_arguments(
                &arguments,
                "aarch64-unknown-linux-gnu",
                Path::new("/workspace"),
                Subcommand::List,
            );
            assert!(has_pair(&list, "--message-format", "json"), "{suite:?}");
            assert!(!has_flag(&list, "--filter-expr"), "{suite:?}");
            assert!(!has_flag(&list, "--profile"), "{suite:?}");
            assert!(!has_flag(&list, "--no-tests"), "{suite:?}");
        }
    }

    fn listing(tests: &[(&str, &str, bool)]) -> Vec<u8> {
        let mut suites: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
        for (package, name, ignored) in tests {
            let suite = suites
                .entry(package)
                .or_insert_with(|| serde_json::json!({"package-name": package, "testcases": {}}));
            suite["testcases"][name] = serde_json::json!({"ignored": ignored});
        }
        serde_json::to_vec(&serde_json::json!({"rust-suites": suites})).expect("listing JSON")
    }

    fn full_listing() -> Vec<u8> {
        let mut tests = Vec::new();
        for (package, families) in guarded_library_families() {
            for family in *families {
                tests.push((*package, format!("{family}probe"), false));
            }
        }
        let borrowed: Vec<(&str, &str, bool)> = tests
            .iter()
            .map(|(package, name, ignored)| (*package, name.as_str(), *ignored))
            .collect();
        listing(&borrowed)
    }

    #[test]
    fn listing_guard_accepts_a_complete_manifest() {
        let selection = validate_library_listing(&full_listing()).expect("complete listing");
        let families: usize = guarded_library_families()
            .iter()
            .map(|(_, families)| families.len())
            .sum();
        assert_eq!(selection.families, families);
        assert_eq!(selection.tests, families);
    }

    #[test]
    fn listing_guard_reports_missing_and_ignored_families() {
        let family = guarded_library_families()[0].1[0];
        let missing = listing(&[(guarded_library_families()[0].0, "unrelated::test", false)]);
        assert!(
            validate_library_listing(&missing)
                .expect_err("missing family")
                .to_string()
                .contains(family)
        );
        let ignored = listing(&[(guarded_library_families()[0].0, family, true)]);
        assert!(
            validate_library_listing(&ignored)
                .expect_err("ignored family")
                .to_string()
                .contains("ignored")
        );
    }

    fn metadata(root_full: &str, specs_full: &str, root_ai: &str, specs_ai: &str) -> Vec<u8> {
        format!(
            r#"{{"packages":[
                {{"name":"fshell","targets":[],"features":{{"full":{root_full},"ai":{root_ai}}}}},
                {{"name":"fshell-specifications","targets":[],"features":{{"full":{specs_full},"ai":{specs_ai}}}}}
            ]}}"#
        )
        .into_bytes()
    }

    #[test]
    fn feature_parity_accepts_mirrored_builtin_forwarding() {
        let output = metadata(
            r#"["ai"]"#,
            r#"["ai"]"#,
            r#"["fshell-builtins/ai","fshell-repl/ai"]"#,
            r#"["fshell-builtins/ai"]"#,
        );
        validate_feature_parity(
            &serde_json::from_slice::<CargoMetadata>(&output).expect("metadata"),
        )
        .expect("parity");
    }

    #[test]
    fn feature_parity_rejects_missing_names_and_forwarding() {
        let missing = metadata(r#"["ai","extract"]"#, r#"["ai"]"#, r#"[]"#, r#"[]"#);
        let error = validate_feature_parity(
            &serde_json::from_slice::<CargoMetadata>(&missing).expect("metadata"),
        )
        .expect_err("missing feature");
        assert!(error.to_string().contains("extract"), "{error}");

        let forward = metadata(
            r#"["ai"]"#,
            r#"["ai"]"#,
            r#"["fshell-builtins/ai"]"#,
            r#"[]"#,
        );
        let error = validate_feature_parity(
            &serde_json::from_slice::<CargoMetadata>(&forward).expect("metadata"),
        )
        .expect_err("missing forwarding");
        assert!(error.to_string().contains("fshell-builtins/ai"), "{error}");
    }

    #[test]
    fn artifact_selection_uses_cargo_records_not_directory_conventions() {
        let output = br#"{"reason":"compiler-artifact","target":{"name":"fshell","kind":["lib"]},"executable":null}
{"reason":"compiler-artifact","target":{"name":"fsh","kind":["bin"]},"executable":"/custom-target/release/fsh","fresh":true}
{"reason":"build-finished","success":true}
"#;
        assert_eq!(
            shell_artifact(output).expect("selected executable"),
            Path::new("/custom-target/release/fsh")
        );
    }

    #[test]
    fn absent_ambiguous_relative_and_malformed_artifacts_fail() {
        assert!(shell_artifact(b"{\"reason\":\"build-finished\"}\n").is_err());
        let duplicate = br#"{"reason":"compiler-artifact","target":{"name":"fsh","kind":["bin"]},"executable":"/one/fsh"}
{"reason":"compiler-artifact","target":{"name":"fsh","kind":["bin"]},"executable":"/two/fsh"}"#;
        assert!(shell_artifact(duplicate).is_err());
        assert!(
            shell_artifact(br#"{"reason":"compiler-artifact","target":{"name":"fsh","kind":["bin"]},"executable":"relative/fsh"}"#).is_err()
        );
        assert!(shell_artifact(b"not a Cargo record").is_err());
    }
}
