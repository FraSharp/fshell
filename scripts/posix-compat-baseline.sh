#!/bin/sh
# Small differential smoke baseline for non-interactive POSIX shell behavior.
# Run from any directory: scripts/posix-compat-baseline.sh [fsh-binary]
set -u

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cases_dir=$repo_dir/scripts/posix-compat-baseline
fsh_bin=${1:-${FSH_BIN:-$repo_dir/target/debug/fsh}}
dash_bin=${DASH_BIN:-$(command -v dash || true)}
bash_bin=${BASH_BIN:-$(command -v bash || true)}

if [ ! -x "$fsh_bin" ]; then
    echo "fsh binary not found or not executable: $fsh_bin" >&2
    echo "Build it with: cargo build" >&2
    exit 2
fi
if [ -z "$dash_bin" ] || [ ! -x "$dash_bin" ]; then
    echo "dash is required as the POSIX reference shell" >&2
    exit 2
fi
if [ -z "$bash_bin" ] || [ ! -x "$bash_bin" ]; then
    echo "bash is required as the second reference shell" >&2
    exit 2
fi

tmp_root=$(mktemp -d "${TMPDIR:-/tmp}/fsh-posix-baseline.XXXXXX") || exit 2
trap 'rm -rf "$tmp_root"' 0 HUP INT TERM

failures=0
count=0

run_one() {
    stdout_file=$1
    stderr_file=$2
    shift 2
    if (cd "$tmp_root" && "$@") >"$stdout_file" 2>"$stderr_file"; then
        status=0
    else
        status=$?
    fi
}

for script_file in "$cases_dir"/*.sh; do
    [ -f "$script_file" ] || continue
    case_name=${script_file##*/}
    case_name=${case_name%.sh}
    expected_out=$cases_dir/$case_name.stdout
    expected_status=$cases_dir/$case_name.status
    if [ ! -f "$expected_out" ] || [ ! -f "$expected_status" ]; then
        echo "INCOMPLETE $case_name: expected .stdout and .status files" >&2
        failures=$((failures + 1))
        continue
    fi
    expected_rc=$(cat "$expected_status")
    script_text=$(cat "$script_file")

    run_one "$tmp_root/dash.out" "$tmp_root/dash.err" "$dash_bin" -c "$script_text"
    dash_rc=$status
    run_one "$tmp_root/bash.out" "$tmp_root/bash.err" "$bash_bin" --posix -c "$script_text"
    bash_rc=$status
    run_one "$tmp_root/fsh.out" "$tmp_root/fsh.err" "$fsh_bin" --posix -c "$script_text"
    fsh_rc=$status
    count=$((count + 1))

    refs_ok=1
    if [ "$dash_rc" -ne "$expected_rc" ] || ! cmp -s "$tmp_root/dash.out" "$expected_out" || [ -s "$tmp_root/dash.err" ]; then
        refs_ok=0
    fi
    if [ "$bash_rc" -ne "$expected_rc" ] || ! cmp -s "$tmp_root/bash.out" "$expected_out" || [ -s "$tmp_root/bash.err" ]; then
        refs_ok=0
    fi
    if [ "$refs_ok" -ne 1 ]; then
        echo "INVALID $case_name: committed expectation disagrees with dash or bash --posix"
        failures=$((failures + 1))
        continue
    fi

    if [ "$fsh_rc" -eq "$expected_rc" ] && cmp -s "$tmp_root/fsh.out" "$expected_out" && [ ! -s "$tmp_root/fsh.err" ]; then
        echo "PASS    $case_name"
    else
        echo "FAIL    $case_name (fsh status $fsh_rc, expected $expected_rc)"
        diff -u "$expected_out" "$tmp_root/fsh.out" || true
        if [ -s "$tmp_root/fsh.err" ]; then
            echo "  fsh stderr:"
            sed 's/^/    /' "$tmp_root/fsh.err"
        fi
        failures=$((failures + 1))
    fi
done

printf '\n%d cases: %d passed, %d failed\n' "$count" "$((count - failures))" "$failures"
[ "$failures" -eq 0 ]
