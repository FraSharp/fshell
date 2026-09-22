// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Unit tests for the semantic core: value parsing, validation, risk, lowering and
//! schema generation.

use crate::action::*;
use crate::intent::*;
use crate::lower::{ShellTarget, lower, lower_fsh, lower_posix};
use crate::platform::{Os, Platform, UtilitySet};
use crate::render::{render_clarification, render_fsh, render_posix};
use crate::spec::all_specs;
use crate::validate::{is_complete, validate, validate_intent};

fn linux_platform() -> Platform {
    Platform::new(
        Os::Linux,
        UtilitySet::of([
            "rg",
            "grep",
            "find",
            "lsof",
            "df",
            "du",
            "free",
            "uname",
            "systemctl",
            "docker",
            "ip",
        ]),
    )
}

fn macos_platform() -> Platform {
    Platform::new(
        Os::MacOs,
        UtilitySet::of([
            "grep",
            "find",
            "lsof",
            "df",
            "du",
            "vm_stat",
            "uname",
            "launchctl",
            "docker",
            "ifconfig",
        ]),
    )
}

// --- intent envelope -------------------------------------------------------

#[test]
fn info_topic_round_trips() {
    let query = InfoQuery::Signal {
        signal: Signal::Kill,
        question: Some("what does SIGKILL do?".to_string()),
    };
    let json = serde_json::to_string(&query).unwrap();
    assert_eq!(
        json,
        r#"{"kind":"signal","signal":"KILL","question":"what does SIGKILL do?"}"#
    );
    let back: InfoQuery = serde_json::from_str(&json).unwrap();
    assert_eq!(back, query);

    let intent = Intent::inform(query);
    let json = serde_json::to_string(&intent).unwrap();
    assert_eq!(
        json,
        r#"{"mode":"inform","info":{"kind":"signal","signal":"KILL","question":"what does SIGKILL do?"}}"#
    );
    let back: Intent = serde_json::from_str(&json).unwrap();
    assert_eq!(back, intent);
}

// --- value parsing ---------------------------------------------------------

#[test]
fn byte_size_parses_units() {
    assert_eq!(ByteSize::parse("500").unwrap().bytes(), 500);
    assert_eq!(ByteSize::parse("1kb").unwrap().bytes(), 1000);
    assert_eq!(ByteSize::parse("500MB").unwrap().bytes(), 500_000_000);
    assert_eq!(ByteSize::parse("1GiB").unwrap().bytes(), 1_073_741_824);
    assert_eq!(ByteSize::parse("1.5MiB").unwrap().bytes(), 1_572_864);
    assert_eq!(ByteSize::parse("2g").unwrap().bytes(), 2_000_000_000);
}

#[test]
fn byte_size_rejects_garbage() {
    assert!(ByteSize::parse("").is_err());
    assert!(ByteSize::parse("lots").is_err());
    assert!(ByteSize::parse("5 parsecs").is_err());
    assert!(ByteSize::parse("-5mb").is_err());
}

#[test]
fn byte_size_kib_and_human() {
    assert_eq!(ByteSize::from_bytes(1_073_741_824).kib(), 1_048_576);
    assert_eq!(ByteSize::from_bytes(1_073_741_824).human(), "1GiB");
}

#[test]
fn time_span_parses_units() {
    assert_eq!(TimeSpan::parse("45s").unwrap().seconds(), 45);
    assert_eq!(TimeSpan::parse("30m").unwrap().seconds(), 1_800);
    assert_eq!(TimeSpan::parse("2h").unwrap().seconds(), 7_200);
    assert_eq!(TimeSpan::parse("7d").unwrap().seconds(), 604_800);
    assert_eq!(TimeSpan::parse("90").unwrap().seconds(), 90);
    assert_eq!(TimeSpan::parse("7d").unwrap().minutes_ceil(), 10_080);
}

// --- serde round-trips -----------------------------------------------------

#[test]
fn byte_size_round_trips_as_integer() {
    let value = ByteSize::parse("500MB").unwrap();
    let json = serde_json::to_string(&value).unwrap();
    assert_eq!(json, "500000000");
    let back: ByteSize = serde_json::from_str(&json).unwrap();
    assert_eq!(back, value);
}

#[test]
fn action_deserializes_from_flat_json() {
    let json = r#"{"kind":"find_files","root":".","extension":"log","min_size":"500MB","modified_within":"7d"}"#;
    let action: Action = serde_json::from_str(json).unwrap();
    match action {
        Action::FindFiles(query) => {
            assert_eq!(query.root.as_deref(), Some("."));
            assert_eq!(query.extension.as_deref(), Some("log"));
            assert_eq!(query.min_size.unwrap().bytes(), 500_000_000);
            assert_eq!(query.modified_within.unwrap().seconds(), 604_800);
            assert_eq!(query.name, None);
        }
        other => panic!("unexpected action: {other:?}"),
    }
}

#[test]
fn tagged_target_and_signal_round_trip() {
    let json = r#"{"kind":"signal_process","target":{"by":"pid","pid":1234},"signal":"KILL"}"#;
    let action: Action = serde_json::from_str(json).unwrap();
    match &action {
        Action::SignalProcess(sp) => {
            assert_eq!(sp.target, Some(ProcessTarget::Pid { pid: 1234 }));
            assert_eq!(sp.signal, Some(Signal::Kill));
        }
        other => panic!("unexpected action: {other:?}"),
    }
    let back = serde_json::to_string(&action).unwrap();
    assert!(back.contains("\"by\":\"pid\""));
}

#[test]
fn unit_struct_action_serializes_as_bare_tag() {
    let json = serde_json::to_string(&Action::MemoryInfo(MemoryInfo {})).unwrap();
    assert_eq!(json, r#"{"kind":"memory_info"}"#);
    let back: Action = serde_json::from_str(&json).unwrap();
    assert_eq!(back, Action::MemoryInfo(MemoryInfo {}));
}

// --- validation ------------------------------------------------------------

#[test]
fn complete_action_has_no_issues() {
    let action = Action::SearchText(SearchText {
        pattern: Some("TODO".to_string()),
        ..Default::default()
    });
    assert!(is_complete(&action));
}

#[test]
fn missing_required_parameter_is_reported() {
    let action = Action::RunContainer(RunContainer {
        ports: vec![PortMapping {
            host_port: Some(8000),
            container_port: None,
        }],
        ..Default::default()
    });
    let issues = validate(&action);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].param(), "image");
    match &issues[0] {
        Issue::Missing { description, .. } => {
            assert!(description.contains("container image"));
        }
        other => panic!("expected missing issue, got {other:?}"),
    }
}

#[test]
fn invalid_size_range_is_reported() {
    let action = Action::FindFiles(FindFiles {
        min_size: Some(ByteSize::from_bytes(1000)),
        max_size: Some(ByteSize::from_bytes(10)),
        ..Default::default()
    });
    let issues = validate(&action);
    assert_eq!(issues.len(), 1);
    assert!(matches!(issues[0], Issue::Invalid { .. }));
}

#[test]
fn port_mapping_without_host_port_is_reported() {
    let action = Action::RunContainer(RunContainer {
        image: Some("nginx".to_string()),
        ports: vec![PortMapping {
            host_port: None,
            container_port: Some(80),
        }],
        ..Default::default()
    });
    let issues = validate(&action);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].param(), "ports[0].host_port");
}

#[test]
fn intent_merges_declared_and_derived_issues() {
    let intent = Intent {
        mode: IntentMode::Perform,
        action: Some(Action::RunContainer(RunContainer::default())),
        info: None,
        issues: vec![Issue::ambiguous(
            "image",
            vec!["nginx".to_string(), "redis".to_string()],
            "several images could match",
        )],
    };
    let issues = validate_intent(&intent);
    assert_eq!(issues.len(), 2);
    assert!(render_clarification(&intent).contains("image"));
}

// --- risk ------------------------------------------------------------------

#[test]
fn risk_classification() {
    assert_eq!(Action::MemoryInfo(MemoryInfo {}).risk(), RiskLevel::Safe);
    assert_eq!(
        Action::SignalProcess(SignalProcess {
            target: Some(ProcessTarget::Pid { pid: 1 }),
            ..Default::default()
        })
        .risk(),
        RiskLevel::Destructive
    );
    // Querying a service is safe; changing one is not.
    assert_eq!(
        Action::ServiceControl(ServiceControl {
            action: Some(ServiceAction::Status),
            name: Some("nginx".to_string()),
        })
        .risk(),
        RiskLevel::Safe
    );
    assert_eq!(
        Action::ServiceControl(ServiceControl {
            action: Some(ServiceAction::Restart),
            name: Some("nginx".to_string()),
        })
        .risk(),
        RiskLevel::Destructive
    );
}

// --- lowering: fsh ---------------------------------------------------------

#[test]
fn find_files_lowers_to_native_ff() {
    let action = Action::FindFiles(FindFiles {
        root: Some("/var/log".to_string()),
        name: Some("*.log".to_string()),
        min_size: Some(ByteSize::parse("500MB").unwrap()),
        modified_within: Some(TimeSpan::parse("7d").unwrap()),
        file_type: Some(FileType::File),
        limit: Some(50),
        ..Default::default()
    });
    assert_eq!(
        render_fsh(&action, &linux_platform()).unwrap(),
        r#"ff "/var/log" "size" ">=" 500000000 "modified" "<" "604800s" "type" "=" "file" | filter (name ~ "^.*\.log$") | limit 50"#
    );
}

#[test]
fn list_processes_uses_native_pipeline_operators() {
    let action = Action::ListProcesses(ListProcesses {
        filter: Some(ProcessFilter {
            min_memory: Some(ByteSize::parse("1GiB").unwrap()),
            ..Default::default()
        }),
        sort: Some(ProcessSort {
            by: ProcessSortBy::Memory,
            descending: true,
        }),
        limit: Some(5),
        all_users: false,
    });
    assert_eq!(
        render_fsh(&action, &linux_platform()).unwrap(),
        "ps | filter (rss > 1048576) | sort rss desc | limit 5"
    );
}

#[test]
fn signal_process_prefers_native_kill_for_term() {
    let term = Action::SignalProcess(SignalProcess {
        target: Some(ProcessTarget::Pid { pid: 1234 }),
        signal: None,
    });
    assert_eq!(render_fsh(&term, &linux_platform()).unwrap(), "kill 1234");

    let kill = Action::SignalProcess(SignalProcess {
        target: Some(ProcessTarget::Pid { pid: 1234 }),
        signal: Some(Signal::Kill),
    });
    assert_eq!(
        render_fsh(&kill, &linux_platform()).unwrap(),
        r#"exec "kill" "-KILL" 1234"#
    );
}

#[test]
fn environment_info_filters_natively() {
    let action = Action::EnvironmentInfo(EnvironmentInfo {
        variable: Some("PATH".to_string()),
    });
    assert_eq!(
        render_fsh(&action, &linux_platform()).unwrap(),
        r#"env | filter (key == "PATH")"#
    );
}

// --- lowering: posix -------------------------------------------------------

#[test]
fn find_files_lowers_to_portable_find() {
    let action = Action::FindFiles(FindFiles {
        root: Some("/var/log".to_string()),
        name: Some("*.log".to_string()),
        min_size: Some(ByteSize::parse("500MB").unwrap()),
        modified_within: Some(TimeSpan::parse("7d").unwrap()),
        file_type: Some(FileType::File),
        limit: Some(50),
        ..Default::default()
    });
    assert_eq!(
        render_posix(&action, &linux_platform()).unwrap(),
        "find '/var/log' -name '*.log' -size +500000000c -mmin -10080 -type f | head -n 50"
    );
}

#[test]
fn list_processes_posix_uses_ps_awk_sort_head() {
    let action = Action::ListProcesses(ListProcesses {
        sort: Some(ProcessSort {
            by: ProcessSortBy::Memory,
            descending: true,
        }),
        limit: Some(5),
        ..Default::default()
    });
    assert_eq!(
        render_posix(&action, &linux_platform()).unwrap(),
        "ps aux | sort -k6 -n -r | head -n 5"
    );
}

#[test]
fn run_container_publishes_ports() {
    let action = Action::RunContainer(RunContainer {
        image: Some("nginx".to_string()),
        ports: vec![PortMapping {
            host_port: Some(8000),
            container_port: None,
        }],
        detached: true,
        ..Default::default()
    });
    assert_eq!(
        render_posix(&action, &linux_platform()).unwrap(),
        "docker run -d -p 8000:8000 'nginx'"
    );
}

// --- platform divergence ---------------------------------------------------

#[test]
fn memory_info_differs_by_platform() {
    let action = Action::MemoryInfo(MemoryInfo {});
    assert_eq!(render_fsh(&action, &macos_platform()).unwrap(), "vm_stat");
    assert_eq!(
        render_fsh(&action, &linux_platform()).unwrap(),
        r#"free "-h""#
    );
}

#[test]
fn service_mutation_is_unsupported_on_macos() {
    let action = Action::ServiceControl(ServiceControl {
        action: Some(ServiceAction::Restart),
        name: Some("nginx".to_string()),
    });
    let error = render_fsh(&action, &macos_platform()).unwrap_err();
    assert!(error.to_string().contains("launchctl"));
}

#[test]
fn listening_ports_falls_back_to_ss_when_lsof_missing() {
    let platform = Platform::new(Os::Linux, UtilitySet::of(["ss"]));
    let action = Action::ListeningPorts(ListeningPorts {
        port: Some(8000),
        ..Default::default()
    });
    assert_eq!(
        render_fsh(&action, &platform).unwrap(),
        r#"ss "-lntup" | grep ":8000""#
    );
}

// --- target dispatch -------------------------------------------------------

#[test]
fn lower_dispatches_by_target() {
    let action = Action::GitStatus(GitStatus { path: None });
    assert!(matches!(
        lower(&action, ShellTarget::Fsh, &linux_platform()).unwrap(),
        crate::Lowered::Fsh(_)
    ));
    assert!(matches!(
        lower(&action, ShellTarget::Posix, &linux_platform()).unwrap(),
        crate::Lowered::Posix(_)
    ));
    assert_eq!(
        render_posix(&action, &linux_platform()).unwrap(),
        "git status --porcelain"
    );
}

#[test]
fn unused_lower_helpers_stay_exercised() {
    // Guard against dead lowering paths when the action set changes.
    let platform = linux_platform();
    for spec in all_specs() {
        let action = sample_for(spec.kind);
        if let Some(action) = action {
            assert_eq!(action.kind(), spec.kind);
            let _ = lower_fsh(&action, &platform);
            let _ = lower_posix(&action, &platform);
        }
    }
}

/// A minimal, fully-populated example for a kind (used to smoke-test every lowering path).
pub(crate) fn sample_for(kind: &str) -> Option<Action> {
    Some(match kind {
        "list_processes" => Action::ListProcesses(ListProcesses::default()),
        "inspect_process" => Action::InspectProcess(InspectProcess { pid: Some(1) }),
        "signal_process" => Action::SignalProcess(SignalProcess {
            target: Some(ProcessTarget::Pid { pid: 1 }),
            signal: Some(Signal::Term),
        }),
        "memory_info" => Action::MemoryInfo(MemoryInfo {}),
        "system_info" => Action::SystemInfo(SystemInfo {}),
        "disk_usage" => Action::DiskUsage(DiskUsage::default()),
        "find_files" => Action::FindFiles(FindFiles::default()),
        "search_text" => Action::SearchText(SearchText {
            pattern: Some("x".to_string()),
            ..Default::default()
        }),
        "list_directory" => Action::ListDirectory(ListDirectory::default()),
        "delete_path" => Action::DeletePath(DeletePath {
            path: Some("/tmp/x".to_string()),
            ..Default::default()
        }),
        "listening_ports" => Action::ListeningPorts(ListeningPorts::default()),
        "network_interfaces" => Action::NetworkInterfaces(NetworkInterfaces {}),
        "git_status" => Action::GitStatus(GitStatus::default()),
        "git_branches" => Action::GitBranches(GitBranches::default()),
        "run_container" => Action::RunContainer(RunContainer {
            image: Some("nginx".to_string()),
            ..Default::default()
        }),
        "list_containers" => Action::ListContainers(ListContainers::default()),
        "service_control" => Action::ServiceControl(ServiceControl {
            action: Some(ServiceAction::Status),
            name: Some("nginx".to_string()),
        }),
        "environment_info" => Action::EnvironmentInfo(EnvironmentInfo::default()),
        _ => return None,
    })
}

// --- schema generation -----------------------------------------------------

#[cfg(feature = "schema")]
#[test]
fn tools_are_generated_for_every_action() {
    use crate::schema::tools_json;
    let tools = tools_json();
    let list = tools["tools"].as_array().unwrap();
    assert_eq!(list.len(), all_specs().len());
    assert_eq!(list.len(), 18);

    let names: Vec<&str> = list
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"find_files"));
    assert!(names.contains(&"run_container"));
}

#[cfg(feature = "schema")]
#[test]
fn required_parameters_come_from_the_spec_table() {
    use crate::schema::parameter_schema;
    let find = parameter_schema("find_files").unwrap();
    assert_eq!(find["required"], serde_json::json!([]));
    assert!(find["properties"]["min_size"].is_object());

    let search = parameter_schema("search_text").unwrap();
    assert_eq!(search["required"], serde_json::json!(["pattern"]));

    let signal = parameter_schema("signal_process").unwrap();
    assert_eq!(signal["required"], serde_json::json!(["target"]));
}

#[cfg(feature = "schema")]
#[test]
fn signal_enum_is_exposed_in_the_schema() {
    use crate::schema::parameter_schema;
    let signal = parameter_schema("signal_process").unwrap();
    let rendered = serde_json::to_string(&signal).unwrap();
    assert!(
        rendered.contains("\"KILL\""),
        "schema should enumerate signals: {rendered}"
    );
}
