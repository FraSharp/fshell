//! Artifact-aware orchestration. Nextest owns discovery, scheduling and verdicts.

use clap::{Parser, ValueEnum};
use serde::Deserialize;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Suite {
    /// Portable support, library and executable checks.
    All,
    Support,
    Library,
    Executable,
    /// Linux kernel containment probes; requires user/PID namespaces and pidfds.
    Containment,
}

impl Suite {
    fn needs_shell(self) -> bool {
        matches!(self, Self::All | Self::Executable)
    }

    fn profile(self) -> &'static str {
        match self {
            Self::All => "specifications",
            Self::Support => "support",
            Self::Library => "library",
            Self::Executable => "executable",
            Self::Containment => "containment",
        }
    }
}

// Explicit classification: additions/removals must update the execution plan.
// Cargo metadata is checked before builds, so an unclassified target fails.
const SPECIFICATION_TARGETS: &[(&str, Suite)] = &[
    ("builtin_registry", Suite::Library),
    ("invocation_state", Suite::Library),
    ("native_evaluation", Suite::Library),
    ("evaluation_effects", Suite::Library),
    ("native_parsing", Suite::Library),
    ("pipeline_values", Suite::Library),
    ("posix_expansion", Suite::Library),
    ("search_streams", Suite::Library),
    ("cli_execution", Suite::Executable),
];

#[derive(Debug, Parser)]
#[command(about = "Run fshell specifications with explicit artifacts and per-test isolation")]
struct Arguments {
    #[arg(long, value_enum, default_value = "all")]
    suite: Suite,
    /// Build and test all optional shell features.
    #[arg(long)]
    full: bool,
    /// Build the selected artifacts and specifications in release mode.
    #[arg(long)]
    release: bool,
    /// Explicit native Rust target. Cross execution is not supported here.
    #[arg(long)]
    target: Option<String>,
    /// Nextest filter expression. An empty selection fails.
    #[arg(long)]
    filter: Option<String>,
}

fn main() -> ExitCode {
    match run(Arguments::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fshell-test-runner: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Arguments) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.full && matches!(arguments.suite, Suite::Support | Suite::Containment) {
        return Err(
            io::Error::other("--full is not applicable to mechanical support checks").into(),
        );
    }
    if arguments.suite == Suite::Containment && !cfg!(target_os = "linux") {
        return Err(
            io::Error::other("PID-namespace containment requires a native Linux host").into(),
        );
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let nextest = Command::new(&cargo)
        .args(["nextest", "--version"])
        .current_dir(&root)
        .stdin(Stdio::null())
        .output()?;
    if !nextest.status.success() {
        return Err(io::Error::other("cargo-nextest is required; install with: cargo install cargo-nextest --version 0.9.114 --locked").into());
    }
    io::stdout().write_all(&nextest.stdout)?;
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let version = Command::new(rustc)
        .arg("-vV")
        .stdin(Stdio::null())
        .output()?;
    if !version.status.success() {
        return Err(io::Error::other("could not determine the native compiler target").into());
    }
    let host = native_target(&version.stdout)?;
    let target = validate_target(arguments.target.as_deref(), &host)?;
    if !matches!(arguments.suite, Suite::Support | Suite::Containment) {
        let metadata = Command::new(&cargo)
            .args(["metadata", "--locked", "--no-deps", "--format-version=1"])
            .current_dir(&root)
            .stdin(Stdio::null())
            .output()?;
        io::stderr().write_all(&metadata.stderr)?;
        if !metadata.status.success() {
            return Err(
                io::Error::other(format!("Cargo metadata failed: {}", metadata.status)).into(),
            );
        }
        validate_specification_targets(&metadata.stdout)?;
    }
    let executable = if arguments.suite.needs_shell() {
        let output = Command::new(&cargo)
            .args(build_arguments(&arguments, &target))
            .current_dir(&root)
            .stdin(Stdio::null())
            .output()?;
        io::stderr().write_all(&output.stderr)?;
        if !output.status.success() {
            return Err(io::Error::other(format!("shell build failed: {}", output.status)).into());
        }
        let path = shell_artifact(&output.stdout)?;
        if !path.is_file() {
            return Err(io::Error::other(format!(
                "Cargo artifact does not exist: {}",
                path.display()
            ))
            .into());
        }
        println!("shell artifact: {}", path.display());
        Some(path)
    } else {
        None
    };
    let mut nextest = Command::new(&cargo);
    nextest.args(test_arguments(&arguments, &target, &root));
    nextest.current_dir(&root).stdin(Stdio::null());
    // Artifact-free selections must not accept a caller-supplied executable.
    nextest.env_remove("FSHELL_TEST_EXECUTABLE");
    if let Some(path) = executable {
        nextest.env("FSHELL_TEST_EXECUTABLE", path);
    }
    let status = nextest.status()?;
    if !status.success() {
        return Err(io::Error::other(format!("specifications failed: {status}")).into());
    }
    Ok(())
}

fn build_arguments(arguments: &Arguments, target: &str) -> Vec<OsString> {
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

fn test_arguments(arguments: &Arguments, target: &str, root: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["nextest", "run", "--locked", "--config-file"]
        .into_iter()
        .map(Into::into)
        .collect();
    args.push(root.join("tests/nextest.toml").into_os_string());
    args.extend(
        [
            "--profile",
            arguments.suite.profile(),
            "--no-tests",
            "fail",
            "--target",
            target,
        ]
        .into_iter()
        .map(OsString::from),
    );
    let packages: &[&str] = match arguments.suite {
        Suite::All => &[
            "fshell-test-support",
            "fshell-specifications",
            "fshell-test-runner",
            "fshell-posix",
            "fshell-core",
        ],
        Suite::Support => &["fshell-test-support", "fshell-test-runner"],
        Suite::Library => &["fshell-specifications", "fshell-posix", "fshell-core"],
        Suite::Executable => &["fshell-specifications"],
        Suite::Containment => &["fshell-test-support"],
    };
    for package in packages {
        args.extend(["--package".into(), (*package).into()]);
    }
    match arguments.suite {
        // Include runner unit tests; examples and benchmarks are not selected.
        Suite::All | Suite::Support => {
            args.extend(["--lib".into(), "--bins".into(), "--tests".into()]);
        }
        Suite::Containment => {
            args.extend(["--test".into(), "namespace_containment".into()]);
        }
        Suite::Library | Suite::Executable => {
            if arguments.suite == Suite::Library {
                args.push("--lib".into());
            }
            for (target, suite) in SPECIFICATION_TARGETS {
                if *suite == arguments.suite {
                    args.extend(["--test".into(), (*target).into()]);
                }
            }
        }
    }
    if arguments.release {
        args.push("--release".into());
    }
    if arguments.full {
        args.extend(["--features".into(), "fshell-specifications/full".into()]);
    }
    if let Some(filter) = &arguments.filter {
        args.extend(["--filter-expr".into(), filter.into()]);
    }
    args
}

fn native_target(output: &[u8]) -> io::Result<String> {
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

fn validate_target(requested: Option<&str>, host: &str) -> io::Result<String> {
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

#[derive(Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
}

#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<MetadataPackage>,
}

#[derive(Deserialize)]
struct MetadataPackage {
    name: String,
    targets: Vec<CargoTarget>,
}

fn validate_specification_targets(output: &[u8]) -> io::Result<()> {
    let metadata: CargoMetadata = serde_json::from_slice(output)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let packages: Vec<_> = metadata
        .packages
        .iter()
        .filter(|package| package.name == "fshell-specifications")
        .collect();
    if packages.len() != 1 {
        return Err(io::Error::other(
            "metadata must contain exactly one specifications package",
        ));
    }
    let mut actual: Vec<_> = packages[0]
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

fn shell_artifact(output: &[u8]) -> io::Result<PathBuf> {
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
        let test = test_arguments(
            &arguments,
            "aarch64-unknown-linux-gnu",
            Path::new("/workspace"),
        );
        for args in [&build, &test] {
            assert!(args.contains(&OsString::from("--locked")));
            assert!(args.contains(&OsString::from("--release")));
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--target", "aarch64-unknown-linux-gnu"])
            );
        }
        assert!(build.windows(2).any(|pair| pair == ["--features", "full"]));
        assert!(
            test.windows(2)
                .any(|pair| pair == ["--features", "fshell-specifications/full"])
        );
    }

    #[test]
    fn support_selection_builds_only_support_and_runner_tests() {
        let arguments = Arguments::parse_from(["runner", "--suite", "support"]);
        let args = test_arguments(
            &arguments,
            "aarch64-unknown-linux-gnu",
            Path::new("/workspace"),
        );
        assert!(args.contains(&OsString::from("fshell-test-support")));
        assert!(args.contains(&OsString::from("fshell-test-runner")));
        assert!(!args.contains(&OsString::from("fshell-specifications")));
        assert!(args.windows(2).any(|pair| pair == ["--no-tests", "fail"]));
    }

    #[test]
    fn containment_selection_prepares_only_its_explicit_support_target() {
        let arguments = Arguments::parse_from(["runner", "--suite", "containment"]);
        assert!(!arguments.suite.needs_shell());
        let args = test_arguments(
            &arguments,
            "aarch64-unknown-linux-gnu",
            Path::new("/workspace"),
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
        let args = test_arguments(
            &arguments,
            "aarch64-unknown-linux-gnu",
            Path::new("/workspace"),
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--filter-expr", "test(arithmetic) | test(invocation)"])
        );
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
        assert!(shell_artifact(br#"{"reason":"compiler-artifact","target":{"name":"fsh","kind":["bin"]},"executable":"relative/fsh"}"#).is_err());
        assert!(shell_artifact(b"not a Cargo record").is_err());
    }
}
