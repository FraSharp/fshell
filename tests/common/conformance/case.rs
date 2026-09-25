//! Case definitions for the conformance suite.
//!
//! A case is a shell snippet plus the engines it should be run in and how its
//! correct behaviour is established. Cases are deliberately data, not bespoke
//! test functions, so the corpus stays declarative and extensible.

/// A way of invoking fsh.
///
/// The two engines are not one implementation, so they are named explicitly
/// rather than tested as "fshell": a snippet can be correct in one and wrong in
/// the other, and the suite must say which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Engine {
    /// `--native`: the fsh language only, POSIX fallback disabled.
    Native,
    /// `--posix`: the POSIX frontend.
    Posix,
    /// No flag: fsh dispatches between the native engine and the POSIX fallback.
    /// This is what a coding agent actually gets.
    Auto,
}

impl Engine {
    /// CLI flags selecting this engine.
    pub fn flags(self) -> &'static [&'static str] {
        match self {
            Engine::Native => &["--native"],
            Engine::Posix => &["--posix"],
            Engine::Auto => &[],
        }
    }

    /// How this engine is described in a failure report.
    pub fn label(self) -> &'static str {
        match self {
            Engine::Native => "native (fsh --native)",
            Engine::Posix => "posix (fsh --posix)",
            Engine::Auto => "auto (fsh)",
        }
    }

    /// The name this engine reports in `$FSH_ENGINE_TRACE`.
    pub fn trace_name(self) -> &'static str {
        match self {
            Engine::Native => "native",
            Engine::Posix => "posix",
            // Auto is not an engine but the absence of a choice, so it is never
            // what the router reports having picked.
            Engine::Auto => "auto",
        }
    }
}

/// How the expected behaviour of a case is established.
pub enum Oracle {
    /// Run the installed POSIX reference shells and require them to agree.
    Posix,
    /// Judge against `bash --posix` only.
    ///
    /// For syntax a strict POSIX shell legitimately rejects but bash accepts
    /// even in POSIX mode (here-strings, `[[ … ]]`), requiring `dash` to agree
    /// would turn the case into a reference disagreement instead of a testable
    /// expectation.
    BashPosix,
    /// Judge against plain `bash` (no `--posix`).
    ///
    /// For bash extensions that bash's own POSIX mode rejects, such as process
    /// substitution.
    Bash,
    /// A literal expectation, for fsh-native behaviour no reference shell models.
    ///
    /// `stdout` may contain `{HOME}` or `{ROOT}`, substituted with the fixture
    /// paths at run time so the case stays independent of the temp directory.
    Expect { stdout: &'static str, exit: i32 },
}

/// A case that is currently expected to fail, for a known reason.
///
/// This is a *strict* marker: the listed engines must still produce the wrong
/// result. If one starts passing, the suite fails and demands the marker be
/// removed — so a bug can neither regress silently nor be fixed while the
/// corpus keeps claiming it is broken.
pub struct KnownFailure {
    /// Short identifier, e.g. a bug slug.
    pub id: &'static str,
    /// Why the case fails today.
    pub reason: &'static str,
    /// Exactly which engines are expected to fail.
    pub engines: &'static [Engine],
}

/// One conformance case.
pub struct Case {
    pub name: &'static str,
    pub script: &'static str,
    pub engines: &'static [Engine],
    pub oracle: Oracle,
    /// Feature tags, for the interaction-coverage bookkeeping the spec asks for.
    pub features: &'static [&'static str],
    /// Fixture-relative files whose contents form part of the outcome.
    pub files: &'static [&'static str],
    /// Compare stderr text as well as stdout and status.
    pub compare_stderr: bool,
    pub known_failure: Option<KnownFailure>,
    /// The engine that must actually run the script in auto mode.
    ///
    /// Only consulted for [`Engine::Auto`], where the choice is a runtime
    /// decision — with `--native` or `--posix` the engine is known in advance,
    /// so asserting it would be tautological. This is what stops a case from
    /// passing because auto mode produced the right bytes *through the wrong
    /// engine*.
    pub expected_engine: Option<Engine>,
    /// Strict marker: the routing expectation is known to be unmet today.
    ///
    /// Kept separate from [`KnownFailure`] so a case can report *semantic PASS,
    /// routing XFAIL* — correct output, wrong engine — which a single combined
    /// marker would hide.
    pub known_routing_failure: Option<&'static str>,
}

impl Case {
    /// A POSIX case: checked against the reference shells, run in `--posix` and
    /// in default (auto) mode so a dispatch or native-engine defect is caught
    /// as well as a POSIX-frontend one.
    pub fn posix(name: &'static str, script: &'static str) -> Self {
        Self {
            name,
            script,
            engines: &[Engine::Posix, Engine::Auto],
            oracle: Oracle::Posix,
            features: &[],
            files: &[],
            compare_stderr: false,
            known_failure: None,
            expected_engine: None,
            known_routing_failure: None,
        }
    }

    /// A POSIX case restricted to the POSIX frontend.
    pub fn posix_only(name: &'static str, script: &'static str) -> Self {
        Self::posix(name, script).engines(&[Engine::Posix])
    }

    /// A native-engine case with a literal expectation.
    pub fn native(
        name: &'static str,
        script: &'static str,
        expected_stdout: &'static str,
        expected_exit: i32,
    ) -> Self {
        Self {
            name,
            script,
            engines: &[Engine::Native],
            oracle: Oracle::Expect {
                stdout: expected_stdout,
                exit: expected_exit,
            },
            features: &[],
            files: &[],
            compare_stderr: false,
            known_failure: None,
            expected_engine: None,
            known_routing_failure: None,
        }
    }

    /// Override the engines this case runs in.
    pub fn engines(mut self, engines: &'static [Engine]) -> Self {
        self.engines = engines;
        self
    }

    /// Tag the features this case exercises.
    pub fn features(mut self, features: &'static [&'static str]) -> Self {
        self.features = features;
        self
    }

    /// Capture these fixture-relative files as part of the outcome.
    pub fn files(mut self, files: &'static [&'static str]) -> Self {
        self.files = files;
        self
    }

    /// Include stderr text in the comparison.
    pub fn compare_stderr(mut self) -> Self {
        self.compare_stderr = true;
        self
    }

    /// Judge this case against `bash --posix` only.
    pub fn bash_posix_only(mut self) -> Self {
        self.oracle = Oracle::BashPosix;
        self
    }

    /// Judge this case against plain `bash` only.
    pub fn bash_only(mut self) -> Self {
        self.oracle = Oracle::Bash;
        self
    }

    /// Mark this case as a strict known failure for the given engines.
    pub fn known_failure(
        mut self,
        id: &'static str,
        reason: &'static str,
        engines: &'static [Engine],
    ) -> Self {
        self.known_failure = Some(KnownFailure {
            id,
            reason,
            engines,
        });
        self
    }

    /// Require auto mode to run this script in `engine`.
    pub fn expect_engine(mut self, engine: Engine) -> Self {
        self.expected_engine = Some(engine);
        self
    }

    /// Mark the routing expectation as a strict known failure: the wrong engine
    /// is known to be chosen today, for `reason`.
    pub fn known_routing_failure(mut self, reason: &'static str) -> Self {
        self.known_routing_failure = Some(reason);
        self
    }
}
