// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Derivation of function-calling tool schemas from the semantic types.
//!
//! One tool per [`Action`] kind, with parameters generated from the Rust types themselves
//! via `schemars` and requiredness taken from [`crate::spec`]. The output is the
//! OpenAI-style `tools` array — the de-facto function-calling format — which a thin
//! adapter can reshape into FunctionGemma's exact call syntax. No model-specific concept
//! leaks into the semantic core.

use schemars::schema_for;
use serde_json::{Value, json};

use crate::action::*;
use crate::spec::{ActionSpec, all_specs, spec_for};

/// A single tool definition derived from an action.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSchema {
    /// Tool name (the action kind).
    pub name: String,
    /// One-line description.
    pub description: String,
    /// JSON Schema for the tool's parameters.
    pub parameters: Value,
}

/// Every action as a tool schema.
pub fn functiongemma_tools() -> Vec<ToolSchema> {
    all_specs()
        .into_iter()
        .map(|spec| ToolSchema {
            name: spec.kind.to_string(),
            description: spec.summary.to_string(),
            parameters: parameters_for(&spec),
        })
        .collect()
}

/// The full `{"tools":[...]}` document.
pub fn tools_json() -> Value {
    let tools: Vec<Value> = functiongemma_tools()
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }
            })
        })
        .collect();
    json!({ "tools": tools })
}

/// The parameter schema for a single action kind.
pub fn parameter_schema(kind: &str) -> Option<Value> {
    spec_for(kind).map(|spec| parameters_for(&spec))
}

fn parameters_for(spec: &ActionSpec) -> Value {
    let mut schema =
        action_schema(spec.kind).unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    if let Value::Object(map) = &mut schema {
        map.insert("title".to_string(), json!(spec.kind));
        map.insert("required".to_string(), json!(spec.required));
    }
    schema
}

fn action_schema(kind: &str) -> Option<Value> {
    let schema = match kind {
        "list_processes" => schema_for!(ListProcesses),
        "inspect_process" => schema_for!(InspectProcess),
        "signal_process" => schema_for!(SignalProcess),
        "memory_info" => schema_for!(MemoryInfo),
        "system_info" => schema_for!(SystemInfo),
        "disk_usage" => schema_for!(DiskUsage),
        "find_files" => schema_for!(FindFiles),
        "search_text" => schema_for!(SearchText),
        "list_directory" => schema_for!(ListDirectory),
        "delete_path" => schema_for!(DeletePath),
        "listening_ports" => schema_for!(ListeningPorts),
        "network_interfaces" => schema_for!(NetworkInterfaces),
        "git_status" => schema_for!(GitStatus),
        "git_branches" => schema_for!(GitBranches),
        "run_container" => schema_for!(RunContainer),
        "list_containers" => schema_for!(ListContainers),
        "service_control" => schema_for!(ServiceControl),
        "environment_info" => schema_for!(EnvironmentInfo),
        _ => return None,
    };
    serde_json::to_value(schema).ok()
}
