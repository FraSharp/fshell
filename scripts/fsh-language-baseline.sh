#!/bin/sh
# Golden-output smoke baseline for documented native fsh language behavior.
# Run from any directory: scripts/fsh-language-baseline.sh [fsh-binary]
set -u

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cases_dir=$repo_dir/scripts/fsh-language-baseline
fsh_bin=${1:-${FSH_BIN:-$repo_dir/target/debug/fsh}}

if [ ! -x "$fsh_bin" ]; then
    echo "fsh binary not found or not executable: $fsh_bin" >&2
    echo "Build it with: cargo build" >&2
    exit 2
fi

tmp_root=$(mktemp -d "${TMPDIR:-/tmp}/fsh-language-baseline.XXXXXX") || exit 2
trap 'rm -rf "$tmp_root"' 0 HUP INT TERM

failures=0
count=0

for script_file in "$cases_dir"/*.fsh; do
    [ -f "$script_file" ] || continue
    case_name=${script_file##*/}
    case_name=${case_name%.fsh}
    expected_out=$cases_dir/$case_name.stdout
    expected_status=$cases_dir/$case_name.status
    if [ ! -f "$expected_out" ] || [ ! -f "$expected_status" ]; then
        echo "INCOMPLETE $case_name: expected .stdout and .status files" >&2
        failures=$((failures + 1))
        continue
    fi

    case_dir=$tmp_root/$case_name
    mkdir "$case_dir" || exit 2
    case_script=$case_dir/$case_name.fsh
    cp "$script_file" "$case_script" || exit 2
    script_text=$(cat "$script_file")
    if (cd "$case_dir" && "$fsh_bin" --no-color --error-format compact -c "$script_text") \
        >"$tmp_root/command.out" 2>"$tmp_root/command.err"; then
        command_status=0
    else
        command_status=$?
    fi
    if (cd "$case_dir" && "$fsh_bin" --no-color --error-format compact "$case_script") \
        >"$tmp_root/script.out" 2>"$tmp_root/script.err"; then
        script_status=0
    else
        script_status=$?
    fi
    expected_rc=$(cat "$expected_status")
    count=$((count + 1))

    command_ok=1
    if [ "$command_status" -ne "$expected_rc" ] || ! cmp -s "$tmp_root/command.out" "$expected_out" || [ -s "$tmp_root/command.err" ]; then
        command_ok=0
        echo "  -c status $command_status, expected $expected_rc"
        diff -u "$expected_out" "$tmp_root/command.out" || true
        if [ -s "$tmp_root/command.err" ]; then
            echo "    stderr (-c):"
            sed 's/^/      /' "$tmp_root/command.err"
        fi
    fi
    script_ok=1
    if [ "$script_status" -ne "$expected_rc" ] || ! cmp -s "$tmp_root/script.out" "$expected_out" || [ -s "$tmp_root/script.err" ]; then
        script_ok=0
        echo "  script status $script_status, expected $expected_rc"
        diff -u "$expected_out" "$tmp_root/script.out" || true
        if [ -s "$tmp_root/script.err" ]; then
            echo "    stderr (script):"
            sed 's/^/      /' "$tmp_root/script.err"
        fi
    fi
    if [ "$command_ok" -eq 1 ] && [ "$script_ok" -eq 1 ]; then
        echo "PASS    $case_name"
    else
        echo "FAIL    $case_name"
        failures=$((failures + 1))
    fi
done

printf '\n%d cases: %d passed, %d failed\n' "$count" "$((count - failures))" "$failures"
[ "$failures" -eq 0 ]
