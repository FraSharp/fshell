//! Effective builtin contracts come from the runtime registry, not keyword names.

use fshell_engine::{BuiltinContract, BuiltinHandler, Env, InputPreference};
use std::sync::Arc;

fn registered_environment() -> Env {
    let env = Env::for_command();
    fshell_builtins::init(&env);
    env
}

#[test]
fn grep_requests_bytes_and_is_a_query_while_record_queries_retain_records() {
    let env = registered_environment();
    assert!(env.get_builtin("grep").is_some());
    let grep = env.builtin_contract("grep");
    assert_eq!(grep.input, InputPreference::Bytes);
    assert!(grep.query);
    for name in ["ff", "ls", "pwd", "echo"] {
        assert!(
            env.get_builtin(name).is_some(),
            "missing registered query {name}"
        );
        let contract = env.builtin_contract(name);
        assert_eq!(contract.input, InputPreference::Records, "{name}");
        assert!(contract.query, "{name}");
    }
}

#[test]
fn handler_replacement_does_not_inherit_the_previous_input_contract() {
    let env = registered_environment();
    #[expect(
        clippy::result_large_err,
        reason = "BuiltinHandler fixes the production ShellError return type"
    )]
    let replacement: BuiltinHandler = Arc::new(|_, _, _, _, _| Ok(()));
    env.register_builtin("grep", replacement);
    assert_eq!(env.builtin_contract("grep").input, InputPreference::Records);
    assert!(!env.builtin_contract("grep").query);
}

#[test]
fn alias_function_and_disabled_builtin_each_shadow_query_metadata() {
    let env = registered_environment();
    env.register_alias("grep", "echo alias");
    assert_eq!(env.builtin_contract("grep").input, InputPreference::Records);
    assert!(!env.builtin_contract("grep").query);
    env.remove_alias("grep");
    assert!(env.builtin_contract("grep").query);
    env.fns
        .write()
        .insert("grep".into(), (Vec::new(), None, Vec::new()));
    assert_eq!(env.builtin_contract("grep").input, InputPreference::Records);
    assert!(!env.builtin_contract("grep").query);
    env.fns.write().remove("grep");
    assert!(env.builtin_contract("grep").query);
    env.options.write().disabled_builtins.push("grep".into());
    assert_eq!(env.builtin_contract("grep").input, InputPreference::Records);
    assert!(!env.builtin_contract("grep").query);
}

#[test]
fn explicit_contract_registration_restores_only_the_requested_metadata() {
    let env = Env::for_command();
    #[expect(
        clippy::result_large_err,
        reason = "BuiltinHandler fixes the production ShellError return type"
    )]
    let handler: BuiltinHandler = Arc::new(|_, _, _, _, _| Ok(()));
    env.register_builtin_with_contract(
        "probe-query",
        handler,
        BuiltinContract {
            input: InputPreference::Bytes,
            query: false,
        },
    );
    let contract = env.builtin_contract("probe-query");
    assert_eq!(contract.input, InputPreference::Bytes);
    assert!(!contract.query);
}

#[test]
fn optional_sandbox_and_archive_commands_follow_the_selected_build() {
    let env = registered_environment();
    assert_eq!(
        env.get_builtin("sandbox").is_some(),
        cfg!(feature = "sandbox")
    );
    assert_eq!(
        env.get_builtin("extract").is_some(),
        cfg!(feature = "extract")
    );
}
