//! Env invocation ownership and publication, without CLI or builtin setup.

use fshell_core::Val;
use fshell_engine::Env;

#[test]
fn invocation_clones_share_status_without_publishing_to_the_shell() {
    let shell = Env::for_command();
    shell.set_published_exit_code(11);
    let invocation = shell.begin_invocation();
    let worker = invocation.clone();
    worker.set_exit_code(23);
    assert_eq!(invocation.exit_code(), 23);
    assert_eq!(shell.exit_code(), 11);
    assert_eq!(*shell.prompt.last_exit_code.read(), 11);
    shell.finish_invocation(&invocation);
    assert_eq!(shell.exit_code(), 23);
    assert_eq!(*shell.prompt.last_exit_code.read(), 23);
}

#[test]
fn nested_publication_does_not_update_the_prompt_until_top_level_completion() {
    let shell = Env::for_command();
    shell.set_published_exit_code(7);
    let outer = shell.begin_invocation();
    let inner = outer.begin_invocation();
    inner.set_exit_code(29);
    outer.finish_invocation(&inner);
    assert_eq!(outer.exit_code(), 29);
    assert_eq!(shell.exit_code(), 7);
    assert_eq!(*shell.prompt.last_exit_code.read(), 7);
    shell.finish_invocation(&outer);
    assert_eq!(shell.exit_code(), 29);
    assert_eq!(*shell.prompt.last_exit_code.read(), 29);
}

#[test]
fn sibling_invocations_have_independent_status_but_shared_shell_bindings() {
    let shell = Env::for_command();
    let first = shell.begin_invocation();
    let second = shell.begin_invocation();
    first.set_exit_code(31);
    second.set_exit_code(37);
    first
        .vars
        .write()
        .insert("shared_binding".into(), Val::Int(42));
    assert_eq!(first.exit_code(), 31);
    assert_eq!(second.exit_code(), 37);
    assert_eq!(shell.exit_code(), 0);
    assert_eq!(
        second.vars.read().get("shared_binding"),
        Some(&Val::Int(42))
    );
    assert_eq!(shell.vars.read().get("shared_binding"), Some(&Val::Int(42)));
}
