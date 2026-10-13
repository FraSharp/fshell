//! Private PID-namespace supervisor; never substitutes for shell behavior.

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    fshell_test_support::namespace::supervisor_main()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("PID-namespace containment requires Linux");
    std::process::ExitCode::from(125)
}
