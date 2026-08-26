#![allow(clippy::await_holding_lock, unused_must_use, unused_imports)]
use fshell_core::Val;
use fshell_engine::Env;

fn setup_posix_env() -> Env {
    let env = Env::for_command();
    fshell_engine::populate_env_from_host(&env);
    fshell_builtins::init(&env);
    fshell_bridge::init(&env);
    env
}

async fn run_posix(script: &str, env: &Env) -> i32 {
    let parsed = fshell_posix::parser::parse_posix_script(script).unwrap();
    fshell_posix::eval::eval_source(&parsed, env, &fshell_posix::eval::EvalConfig::default())
        .await
        .unwrap()
}

#[tokio::test]
async fn posix_var_assignment_and_expansion() {
    let env = setup_posix_env();
    let code = run_posix("X=hello; echo $X", &env).await;
    assert_eq!(code, 0);
    assert_eq!(
        env.vars.read().get("X"),
        Some(&Val::String("hello".to_string()))
    );
}

#[tokio::test]
async fn posix_default_value_expansion() {
    let env = setup_posix_env();
    run_posix("echo ${UNSET:-default}", &env).await;
}

#[tokio::test]
async fn posix_string_length() {
    let env = setup_posix_env();
    env.vars
        .write()
        .insert("FOO".to_string(), Val::String("hello".to_string()));
    let code = run_posix("echo ${#FOO}", &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_if_true_branch() {
    let env = setup_posix_env();
    let code = run_posix("if true; then echo yes; fi", &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_if_false_branch() {
    let env = setup_posix_env();
    let code = run_posix("if false; then echo yes; else echo no; fi", &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_for_loop() {
    let env = setup_posix_env();
    let code = run_posix("for i in 1 2 3; do echo $i; done", &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_while_loop() {
    let env = setup_posix_env();
    let code = run_posix(
        "i=0; while [ $i -lt 3 ]; do i=$((i+1)); done; echo $i",
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_case_statement() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"x=hello; case $x in hello) echo matched;; *) echo no;; esac"#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_and_or_logic() {
    let env = setup_posix_env();
    assert_eq!(run_posix("true && echo ok", &env).await, 0);
    assert_eq!(run_posix("false || echo ok", &env).await, 0);
    assert_eq!(run_posix("false && echo no; echo after", &env).await, 0);
}

#[tokio::test]
async fn posix_subshell_isolation() {
    let env = setup_posix_env();
    env.vars
        .write()
        .insert("X".to_string(), Val::String("outer".to_string()));
    run_posix("(X=inner; echo $X)", &env).await;
    assert_eq!(
        env.vars.read().get("X"),
        Some(&Val::String("outer".to_string()))
    );
}

#[tokio::test]
async fn posix_subshell_cwd_isolation() {
    let env = setup_posix_env();
    let initial_cwd = env.cwd();
    let parent_dir = initial_cwd.parent().unwrap_or(&initial_cwd);

    // Grant read capability for parent
    env.caps
        .caps
        .write()
        .grant(fshell_core::ResourceHandle::ReadDir(
            parent_dir.to_path_buf(),
        ));

    run_posix("(cd ..; pwd)", &env).await;
    assert_eq!(
        env.cwd(),
        initial_cwd,
        "subshell cd must not mutate parent cwd"
    );
}

#[tokio::test]
async fn posix_pipeline() {
    let env = setup_posix_env();
    let code = run_posix("echo hello | cat", &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_pipeline_streaming_early_exit() {
    let env = setup_posix_env();
    let code = run_posix("yes | head -n 5 > /dev/null", &env).await;
    assert_eq!(
        code, 0,
        "yes piped to head -n 5 must terminate with exit code 0"
    );
}

#[tokio::test]
async fn posix_export() {
    let env = setup_posix_env();
    run_posix("export FOO=bar; echo $FOO", &env).await;
    assert_eq!(
        env.vars.read().get("FOO"),
        Some(&Val::String("bar".to_string()))
    );
}

#[tokio::test]
async fn posix_unset() {
    let env = setup_posix_env();
    env.vars
        .write()
        .insert("FOO".to_string(), Val::String("bar".to_string()));
    run_posix("unset FOO", &env).await;
    assert!(env.vars.read().get("FOO").is_none());
}

#[tokio::test]
async fn posix_test_builtin() {
    let env = setup_posix_env();
    assert_eq!(run_posix("test -n hello", &env).await, 0);
    assert_eq!(run_posix("test -z \"\"", &env).await, 0);
    assert_eq!(run_posix("[ -n hello ]", &env).await, 0);
    assert_eq!(run_posix("test 5 -eq 5", &env).await, 0);
    assert_eq!(run_posix("test 5 -lt 10", &env).await, 0);
    assert_eq!(run_posix("test a = a", &env).await, 0);
    assert_eq!(run_posix("test a != b", &env).await, 0);
}

#[tokio::test]
async fn posix_bang_inversion() {
    let env = setup_posix_env();
    assert_eq!(run_posix("! false", &env).await, 0);
    assert_eq!(run_posix("! true", &env).await, 1);
}

#[tokio::test]
async fn posix_function_definition_and_call() {
    let env = setup_posix_env();
    let code = run_posix("myfunc() { echo hello; }; myfunc", &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_ifs_splitting() {
    let env = setup_posix_env();
    let code = run_posix("IFS=:; x=\"a:b:c\"; echo $x", &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_command_substitution() {
    let env = setup_posix_env();
    let code = run_posix("X=$(echo hello); echo $X", &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_arithmetic_expansion() {
    let env = setup_posix_env();
    let code = run_posix("X=$((1 + 2)); echo $X", &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_break_continue() {
    let env = setup_posix_env();
    let code = run_posix(
        "for i in 1 2 3 4 5; do if [ $i -eq 3 ]; then break; fi; echo $i; done",
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_until_loop() {
    let env = setup_posix_env();
    let code = run_posix("i=0; until [ $i -ge 3 ]; do i=$((i+1)); done", &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_shift() {
    let env = setup_posix_env();
    let code = run_posix("set -- a b c; shift; echo $1", &env).await;
    assert_eq!(code, 0);
    assert_eq!(
        env.vars.read().get("1"),
        Some(&Val::String("b".to_string()))
    );
}

#[tokio::test]
async fn posix_set_positional() {
    let env = setup_posix_env();
    run_posix("set -- foo bar baz; echo $1 $2 $3", &env).await;
    assert_eq!(
        env.vars.read().get("1"),
        Some(&Val::String("foo".to_string()))
    );
}

#[tokio::test]
async fn posix_eval_builtin() {
    let env = setup_posix_env();
    let code = run_posix("eval 'X=hello; echo $X'", &env).await;
    assert_eq!(code, 0);
}

#[test]
fn posix_shebang_detection() {
    assert!(fshell_posix::parser::is_posix_shebang("#!/bin/sh\necho hi"));
    assert!(fshell_posix::parser::is_posix_shebang(
        "#!/bin/bash\necho hi"
    ));
    assert!(!fshell_posix::parser::is_posix_shebang(
        "#!/usr/bin/env fsh\necho hi"
    ));
}

#[test]
fn posix_ifs_split_unit() {
    assert_eq!(
        fshell_posix::expand::split_ifs("a  b\tc\n", " \t\n"),
        vec!["a", "b", "c"]
    );
    assert_eq!(
        fshell_posix::expand::split_ifs("a:b::c", ":"),
        vec!["a", "b", "", "c"]
    );
}

#[tokio::test]
async fn posix_pipeline_streaming_filter() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"res=$(printf "%s\n" apple banana cherry | grep an); test "$res" = "banana""#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_pipeline_builtin_streaming() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"res=$(printf "%s\n" 30 10 20 | sort -n | head -n 1); test "$res" = "10""#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_pipeline_function_streaming() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"gen() { echo alpha; echo beta; echo gamma; }; res=$(gen | grep bet); test "$res" = "beta""#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_pipeline_compound_streaming() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"res=$({ echo line1; echo line2; } | grep 2); test "$res" = "line2""#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_pipeline_subshell_scoping() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"VAR=initial; echo hello | { VAR=mutated; }; test "$VAR" = "initial""#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_redirect_stdout_truncate_and_read() {
    let env = setup_posix_env();
    let tmp = std::env::temp_dir().join("posix_test_trunc.txt");
    let tmp_path = tmp.display().to_string();
    let script = format!(
        r#"echo "hello world" > "{}"; res=$(cat "{}"); test "$res" = "hello world""#,
        tmp_path, tmp_path
    );
    let code = run_posix(&script, &env).await;
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_redirect_stdout_append() {
    let env = setup_posix_env();
    let tmp = std::env::temp_dir().join("posix_test_append.txt");
    let tmp_path = tmp.display().to_string();
    let script = format!(
        r#"echo "line1" > "{}"; echo "line2" >> "{}"; count=$(wc -l < "{}"); test "$count" -ge 2"#,
        tmp_path, tmp_path, tmp_path
    );
    let code = run_posix(&script, &env).await;
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_redirect_compound_block() {
    let env = setup_posix_env();
    let tmp = std::env::temp_dir().join("posix_test_compound.txt");
    let tmp_path = tmp.display().to_string();
    let script = format!(
        r#"{{ echo foo; echo bar; }} > "{}"; count=$(wc -l < "{}"); test "$count" -ge 2"#,
        tmp_path, tmp_path
    );
    let code = run_posix(&script, &env).await;
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_builtin_printf_format_specifiers() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"res=$(printf "%s-%04d-%x" item 42 255); test "$res" = "item-0042-ff""#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_builtin_printf_recycling() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"res=$(printf "(%s)" a b c); test "$res" = "(a)(b)(c)""#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_builtin_printf_b_escape() {
    let env = setup_posix_env();
    let script = "res=$(printf '%b' 'hello\\tworld')\nexpected=$(printf 'hello\\tworld')\ntest \"$res\" = \"$expected\"";
    let code = run_posix(script, &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_builtin_getopts_basic() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"
OPTIND=1
opts=""
while getopts "ab" opt -a -b; do
  opts="${opts}${opt}"
done
test "$opts" = "ab"
"#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_builtin_getopts_with_arg() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"
OPTIND=1
while getopts "f:" opt -f myfile; do
  test "$opt" = "f" && test "$OPTARG" = "myfile"
done
"#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_builtin_type_identification() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"
my_dummy_fn() { :; }
type my_dummy_fn | grep -q function
type echo | grep -q builtin
type if | grep -q keyword
"#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_builtin_read_remainder_splitting() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"
echo "one two three four" | {
  read -r first second rest
  test "$first" = "one" && test "$second" = "two" && test "$rest" = "three four"
}
"#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_arithmetic_assignments_and_ternary() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"
val=$((x = 10, y = 20, x > 5 ? y * 2 : 0))
test "$val" -eq 40
"#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_param_expansion_prefix_suffix() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"
path="/usr/local/bin/fsh"
dir="${path%/*}"
name="${path##*/}"
test "$dir" = "/usr/local/bin" && test "$name" = "fsh"
"#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_heredoc_unquoted_expansion() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"
MYVAR="expanded_value"
cat << EOF > /tmp/posix_heredoc_unquoted.txt
hello $MYVAR
EOF
res=$(cat /tmp/posix_heredoc_unquoted.txt)
rm -f /tmp/posix_heredoc_unquoted.txt
test "$res" = "hello expanded_value"
"#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_heredoc_quoted_literal() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"
MYVAR="expanded_value"
cat << 'EOF' > /tmp/posix_heredoc_quoted.txt
hello $MYVAR
EOF
res=$(cat /tmp/posix_heredoc_quoted.txt)
rm -f /tmp/posix_heredoc_quoted.txt
test "$res" = 'hello $MYVAR'
"#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_heredoc_tab_stripping() {
    let env = setup_posix_env();
    let script = "cat <<- EOF > /tmp/posix_heredoc_tabs.txt\n\thello\n\tworld\nEOF\ncount=$(wc -l < /tmp/posix_heredoc_tabs.txt)\nrm -f /tmp/posix_heredoc_tabs.txt\ntest \"$count\" -ge 2";
    let code = run_posix(script, &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_herestring() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"
res=$(cat <<< "herestring content")
test "$res" = "herestring content"
"#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_builtin_getopts_silent_mode() {
    let env = setup_posix_env();
    let script = r#"
OPTIND=1
while getopts ":a:b" opt -x -a myval; do
  case "$opt" in
    \?) bad="$OPTARG";;
    a) a_val="$OPTARG";;
  esac
done
test "$bad" = "x" && test "$a_val" = "myval"
"#;
    let code = run_posix(script, &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_builtin_cd_and_pwd() {
    let env = setup_posix_env();
    let code = run_posix(
        r#"
initial=$(pwd)
cd /tmp
test "$(pwd)" = "/tmp" || test "$(pwd)" = "/private/tmp"
cd "$initial"
test "$(pwd)" = "$initial"
"#,
        &env,
    )
    .await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_subshell_fd_and_env_isolation() {
    let env = setup_posix_env();
    let tmp = std::env::temp_dir().join(format!("subshell_isolation_{}.txt", std::process::id()));
    let tmp_path = tmp.display().to_string();
    let script = format!(
        r#"
(
  X=inner
  cd /tmp
  echo "inside subshell" > "{}"
)
test -z "$X"
"#,
        tmp_path
    );
    let code = run_posix(&script, &env).await;
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(code, 0);
}

#[tokio::test]
async fn posix_failed_input_redirection_aborts() {
    let env = setup_posix_env();
    let code = run_posix("cat < /nonexistent_path_xyz_123_456", &env).await;
    assert_ne!(
        code, 0,
        "reading from nonexistent file must return non-zero exit code"
    );
}
