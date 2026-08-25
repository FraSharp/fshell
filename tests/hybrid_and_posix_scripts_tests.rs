#![allow(clippy::await_holding_lock, unused_must_use, unused_imports)]
use fshell_core::Val;
use fshell_engine::Env;

fn setup_test_env() -> Env {
    let env = Env::for_command();
    fshell_engine::populate_env_from_host(&env);
    fshell_builtins::init(&env);
    fshell_bridge::init(&env);
    fshell_engine::register_posix_handler(
        |content: String, args: Vec<String>, env: Env, capture: bool| async move {
            let parsed = fshell_posix::parser::parse_posix_script(&content)?;
            let cfg = fshell_posix::eval::EvalConfig {
                positional: args,
                ..Default::default()
            };
            fshell_posix::eval::eval_source_stream(&parsed, &env, &cfg, capture).await
        },
    );
    env
}

async fn run_posix(script: &str, env: &Env) -> i32 {
    let parsed = fshell_posix::parser::parse_posix_script(script).unwrap();
    fshell_posix::eval::eval_source(&parsed, env, &fshell_posix::eval::EvalConfig::default())
        .await
        .unwrap()
}

async fn run_fsh(script: &str, env: &Env) -> Result<(), fshell_engine::EngineError> {
    fshell_engine::run_script(script, env).await
}

// -----------------------------------------------------------------------------
// 5 STANDALONE POSIX SCRIPTS
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_posix_suite_1_basic() {
    let env = setup_test_env();
    let script = r#"
set -e
VAR="production_database_replica"
test "${VAR#production_}" = "database_replica"
test "${VAR##*_}" = "replica"
test "${VAR%_replica}" = "production_database"
test "${VAR%%_*}" = "production"
test "${#VAR}" -eq 27
test "${UNSET_VAR:-fallback}" = "fallback"

X=15
Y=30
RESULT=$(( Z = (X * 2 == Y) ? Y + 10 : 0, Z * 2 ))
test "$RESULT" -eq 80

ACC=0
for i in 1 2 3 4 5; do
    ACC=$((ACC + i))
done
test "$ACC" -eq 15

COUNT=5
WHILE_ACC=0
while [ "$COUNT" -gt 0 ]; do
    WHILE_ACC=$((WHILE_ACC + COUNT))
    COUNT=$((COUNT - 1))
done
test "$WHILE_ACC" -eq 15

UNTIL_COUNT=0
until [ "$UNTIL_COUNT" -ge 4 ]; do
    UNTIL_COUNT=$((UNTIL_COUNT + 1))
done
test "$UNTIL_COUNT" -eq 4

IFS=":"
DATA="alpha:beta:gamma:delta"
set -- $DATA
test "$1" = "alpha"
test "$2" = "beta"
test "$3" = "gamma"
test "$4" = "delta"
test "$#" -eq 4
IFS=" \t\n"

test -n "hello"
test ! -z "world"
test 10 -lt 20 -a 20 -gt 10
test 5 -le 5 -a 5 -ge 5
test "abc" != "def"
"#;
    let code = run_posix(script, &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn test_posix_suite_2_pipelines_redirects() {
    let env = setup_test_env();
    let script = r#"
set -e
TMP_DIR=$(mktemp -d /tmp/posix_s2_test_XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

res=$(printf "%s\n" 50 10 40 20 30 | sort -n | grep -E '^[1-4]0$' | head -n 3 | tr '\n' ',')
test "$res" = "10,20,30,"

{
    echo "header_row"
    echo "data_row_1"
    echo "data_row_2"
} > "$TMP_DIR/compound_out.txt"

count=$(wc -l < "$TMP_DIR/compound_out.txt" | tr -d ' ')
test "$count" -ge 3

ITEM="widget"
PRICE="99"
cat << EOF > "$TMP_DIR/receipt.txt"
item: $ITEM
price: \$$PRICE
EOF

test "$(grep 'item:' "$TMP_DIR/receipt.txt")" = "item: widget"
test "$(grep 'price:' "$TMP_DIR/receipt.txt")" = 'price: $99'

cat <<- EOF > "$TMP_DIR/tab_test.txt"
	line_one
	line_two
EOF
test "$(wc -l < "$TMP_DIR/tab_test.txt" | tr -d ' ')" -ge 2

herestring_out=$(cat <<< "direct_stream_input")
test "$herestring_out" = "direct_stream_input"

PARENT_VAL="unchanged"
(
    PARENT_VAL="modified_in_subshell"
    cd "$TMP_DIR"
    test "$PARENT_VAL" = "modified_in_subshell"
)
test "$PARENT_VAL" = "unchanged"

producer() {
    printf "msg_alpha\n"
    printf "msg_beta\n"
    printf "msg_gamma\n"
}
consumer_out=$(producer | grep beta)
test "$consumer_out" = "msg_beta"
"#;
    let code = run_posix(script, &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn test_posix_suite_3_build_deploy() {
    let env = setup_test_env();
    let script = r##"
set -e
BUILD_DIR=$(mktemp -d /tmp/posix_s3_test_XXXXXX)
trap 'rm -rf "$BUILD_DIR"' EXIT

TARGET="release"
VERBOSE=0
PREFIX="/usr/local"
VERSION="1.0.0"

OPTIND=1
while getopts "t:p:vV:" opt -t debug -p /opt/app -v -V 2.1.0; do
    case "$opt" in
        t) TARGET="$OPTARG" ;;
        p) PREFIX="$OPTARG" ;;
        v) VERBOSE=1 ;;
        V) VERSION="$OPTARG" ;;
        \?) exit 1 ;;
    esac
done

test "$TARGET" = "debug"
test "$PREFIX" = "/opt/app"
test "$VERBOSE" -eq 1
test "$VERSION" = "2.1.0"

mkdir -p "$BUILD_DIR/bin" "$BUILD_DIR/etc"
printf "app_name=%s\nversion=%s\ntarget=%s\nprefix=%s\n" "fshell" "$VERSION" "$TARGET" "$PREFIX" > "$BUILD_DIR/manifest.ini"

test -f "$BUILD_DIR/manifest.ini"
test "$(grep 'version=' "$BUILD_DIR/manifest.ini")" = "version=2.1.0"
test "$(grep 'target=' "$BUILD_DIR/manifest.ini")" = "target=debug"

printf "#!/bin/sh\necho 'running %s v%s'\n" "fshell" "$VERSION" > "$BUILD_DIR/bin/fshell_stub"
chmod +x "$BUILD_DIR/bin/fshell_stub"
test -x "$BUILD_DIR/bin/fshell_stub"

build_helper() {
    echo "helper_ok"
}
is_func=$(type build_helper | grep -c "function")
test "$is_func" -ge 1
"##;
    let code = run_posix(script, &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn test_posix_suite_4_data_processor() {
    let env = setup_test_env();
    let script = r#"
set -e
TMP_DIR=$(mktemp -d /tmp/posix_s4_test_XXXXXX)
trap 'rm -rf "$TMP_DIR"' EXIT

CSV_FILE="$TMP_DIR/servers.csv"
REPORT_FILE="$TMP_DIR/report.txt"

cat << 'EOF' > "$CSV_FILE"
srv-web-01,us-east,running,128
srv-web-02,us-west,stopped,64
srv-db-01,us-east,running,512
srv-db-02,eu-central,running,512
srv-cache-01,us-east,running,256
EOF

TOTAL_RAM=0
RUNNING_COUNT=0

while IFS="," read -r name region status ram; do
    if [ "$status" = "running" ]; then
        RUNNING_COUNT=$((RUNNING_COUNT + 1))
        TOTAL_RAM=$((TOTAL_RAM + ram))
    fi
done < "$CSV_FILE"

test "$RUNNING_COUNT" -eq 4
test "$TOTAL_RAM" -eq 1408

printf "SUMMARY_RUNNING=%04d\nSUMMARY_RAM=%d_GB\n" "$RUNNING_COUNT" "$TOTAL_RAM" > "$REPORT_FILE"
test "$(grep 'SUMMARY_RUNNING=' "$REPORT_FILE")" = "SUMMARY_RUNNING=0004"
test "$(grep 'SUMMARY_RAM=' "$REPORT_FILE")" = "SUMMARY_RAM=1408_GB"

top_east_ram=$(grep 'us-east' "$CSV_FILE" | cut -d',' -f4 | sort -n | tail -n 1)
test "$top_east_ram" = "512"
"#;
    let code = run_posix(script, &env).await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn test_posix_suite_5_recursion_matrices() {
    let env = setup_test_env();
    let script = r#"
set -e
factorial() {
    n="$1"
    if [ "$n" -le 1 ]; then
        echo 1
    else
        sub=$((n - 1))
        prev=$(factorial "$sub")
        echo $((n * prev))
    fi
}

fact_5=$(factorial 5)
test "$fact_5" -eq 120

fib() {
    n="$1"
    if [ "$n" -le 0 ]; then
        echo 0
    elif [ "$n" -eq 1 ]; then
        echo 1
    else
        f1=$(fib $((n - 1)))
        f2=$(fib $((n - 2)))
        echo $((f1 + f2))
    fi
}

fib_6=$(fib 6)
test "$fib_6" -eq 8

MATRIX="1:2:3|4:5:6|7:8:9"
DIAG_SUM=0
OLD_IFS="$IFS"

IFS="|"
for row in $MATRIX; do
    IFS=":"
    set -- $row
    case "$1" in
        1) DIAG_SUM=$((DIAG_SUM + $1)) ;;
        4) DIAG_SUM=$((DIAG_SUM + $2)) ;;
        7) DIAG_SUM=$((DIAG_SUM + $3)) ;;
    esac
    IFS="|"
done
IFS="$OLD_IFS"

test "$DIAG_SUM" -eq 15

classify_file() {
    path="$1"
    case "$path" in
        *.tar.gz|*.tgz) echo "compressed_archive" ;;
        *.rs|*.c|*.go)  echo "source_code" ;;
        *.json|*.yaml)  echo "config_file" ;;
        *)              echo "unknown" ;;
    esac
}

test "$(classify_file "release.tar.gz")" = "compressed_archive"
test "$(classify_file "main.rs")" = "source_code"
test "$(classify_file "settings.json")" = "config_file"
test "$(classify_file "README.md")" = "unknown"
"#;
    let code = run_posix(script, &env).await;
    assert_eq!(code, 0);
}

// -----------------------------------------------------------------------------
// 5 HYBRID FSH + SH SCRIPTS
// -----------------------------------------------------------------------------

#[tokio::test]
async fn test_hybrid_suite_1_vars_and_env() {
    let env = setup_test_env();
    let script = r#"
let greeting = "hello_from_fsh"
let counter = 100

sh {
    test "$greeting" = "hello_from_fsh"
    test "$counter" -eq 100
    export HYBRID_EXPORTED="posix_val_42"
    MUTATED_VAR="mutated_by_sh"
}

if $HYBRID_EXPORTED != "posix_val_42" {
    exit 1
}
if $MUTATED_VAR != "mutated_by_sh" {
    exit 1
}
"#;
    run_fsh(script, &env).await.unwrap();
}

#[tokio::test]
async fn test_hybrid_suite_2_streaming_interop() {
    let env = setup_test_env();
    let script = r#"
let tmp_dir = "/tmp/fsh_hybrid_s2_test"

sh {
    mkdir -p /tmp/fsh_hybrid_s2_test
    printf "service_alpha,active,10\nservice_beta,inactive,20\nservice_gamma,active,30\n" > /tmp/fsh_hybrid_s2_test/services.txt
}

let raw_data = $(cat /tmp/fsh_hybrid_s2_test/services.txt)

sh {
    active_count=$(grep ',active,' /tmp/fsh_hybrid_s2_test/services.txt | wc -l | tr -d ' ')
    test "$active_count" -ge 2
    rm -rf /tmp/fsh_hybrid_s2_test
}

if $raw_data == "" {
    exit 1
}
"#;
    run_fsh(script, &env).await.unwrap();
}

#[tokio::test]
async fn test_hybrid_suite_3_caps_and_errors() {
    let env = setup_test_env();
    let script = r#"
let caught_error = false

try {
    sh {
        test "apple" = "banana"
        exit 42
    }
} catch |err| {
    let caught_error = true
}

if !$caught_error {
    exit 1
}

sh {
    val=$((100 + 200))
    test "$val" -eq 300
}
"#;
    run_fsh(script, &env).await.unwrap();
}

#[tokio::test]
async fn test_hybrid_suite_4_fn_composition() {
    let env = setup_test_env();
    let script = r#"
fn compute_package_hash(pkg_name, version) {
    let hash_val = ""
    sh {
        raw_token=$(printf "%s-%s" "$pkg_name" "$version")
        export hash_val=$(echo "$raw_token" | tr '-' '_')
    }
    let res = {
        pkg: $pkg_name,
        ver: $version,
        token: $hash_val
    }
    return $res
}

let result = compute_package_hash "fshell_core" "0.2.0"

if $result.pkg != "fshell_core" {
    exit 1
}
if $result.ver != "0.2.0" {
    exit 1
}
if $result.token != "fshell_core_0.2.0" {
    exit 1
}
"#;
    run_fsh(script, &env).await.unwrap();
}

#[tokio::test]
async fn test_hybrid_suite_5_full_orchestration() {
    let env = setup_test_env();
    let script = r#"
let build_config = {
    target: "release",
    arch: "aarch64",
    opt_level: 3,
    out_dir: "/tmp/fsh_hybrid_s5_test_out"
}

let target = $build_config.target
let arch = $build_config.arch
let opt_level = $build_config.opt_level
let out_dir = $build_config.out_dir

sh {
    mkdir -p /tmp/fsh_hybrid_s5_test_out
    cat << EOF > /tmp/fsh_hybrid_s5_test_out/env.config
TARGET=$target
ARCH=$arch
OPT=$opt_level
EOF
}

sh {
    test -f /tmp/fsh_hybrid_s5_test_out/env.config
    opt_val=$(grep 'OPT=' /tmp/fsh_hybrid_s5_test_out/env.config | cut -d'=' -f2)
    test "$opt_val" = "3"
    rm -rf /tmp/fsh_hybrid_s5_test_out
}
"#;
    run_fsh(script, &env).await.unwrap();
}

#[tokio::test]
async fn test_posix_in_process_sourcing_and_fn_calling() {
    let env = setup_test_env();
    let temp_script = std::env::temp_dir().join(format!("fsh_test_venv_{}.sh", std::process::id()));
    let script_content = r#"
export VIRTUAL_ENV="/custom/venv/path"
export _OLD_PATH="$PATH"
export PATH="$VIRTUAL_ENV/bin:$PATH"

deactivate_venv() {
    export PATH="$_OLD_PATH"
    unset VIRTUAL_ENV
    unset _OLD_PATH
}
"#;
    std::fs::write(&temp_script, script_content).unwrap();

    let fsh_code = format!(
        r#"
source "{}"
"#,
        temp_script.to_string_lossy()
    );

    run_fsh(&fsh_code, &env).await.unwrap();

    // Verify in-process variable mutation
    let venv_val = env.vars.read().get("VIRTUAL_ENV").cloned();
    assert_eq!(venv_val, Some(Val::String("/custom/venv/path".to_string())));

    // Execute the POSIX function defined in the sourced script from fsh
    run_fsh("deactivate_venv", &env).await.unwrap();

    let venv_after = env.vars.read().get("VIRTUAL_ENV").cloned();
    assert!(venv_after.is_none() || venv_after == Some(Val::Null));

    let _ = std::fs::remove_file(temp_script);
}
