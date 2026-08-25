# fshell builtin manual test script
# Run in interactive mode: source tests/builtin-manual-test.fsh
# Or copy-paste sections manually

echo "=== FSHELL BUILTIN TEST SUITE ==="
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 1. FILE SYSTEM
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 1. FILE SYSTEM ━━━"

# cd
echo "--- cd ---"
cd /tmp
pwd
cd ~
pwd

# ls (should show icons, colors, git status)
echo "--- ls ---"
ls
ls /tmp
ls ~ | head -n 3

# pwd
echo "--- pwd ---"
pwd

# mkdir / touch / rm
echo "--- mkdir/touch/rm ---"
mkdir -p /tmp/fshell_test_dir
touch /tmp/fshell_test_dir/test_file.txt
ls /tmp/fshell_test_dir
rm -rf /tmp/fshell_test_dir
echo "mkdir/touch/rm: OK"

# cat
echo "--- cat ---"
echo "hello world" > /tmp/fshell_test_cat.txt
cat /tmp/fshell_test_cat.txt
rm /tmp/fshell_test_cat.txt

# head / tail
echo "--- head/tail ---"
seq 20 > /tmp/fshell_test_seq.txt
head -n 5 /tmp/fshell_test_seq.txt
tail -n 5 /tmp/fshell_test_seq.txt
rm /tmp/fshell_test_seq.txt

# which
echo "--- which ---"
which ls
which echo

# files (NEW builtin)
echo "--- files ---"
files src | head -n 3
files . | filter extension == "rs" | head -n 3

echo ""
echo "━━━ FILE SYSTEM: DONE ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 2. TEXT / STRING
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 2. TEXT / STRING ━━━"

# echo
echo "--- echo ---"
echo "hello world"
echo -n "no newline"
echo ""

# printf
echo "--- printf ---"
printf "name: %s, age: %d\n" "Alice" 30

# string (subcommands: length, upper, lower, trim, contains, starts, ends, index, replace, substring, reverse, repeat, pad)
echo "--- string ---"
string length "hello"
string upper "hello"
string lower "HELLO"
string trim "  spaces  "
string contains "hello world" "world"
string starts "hello" "hel"
string ends "hello" "llo"
string replace "hello world" "world" "fshell"
string reverse "hello"
string repeat "ab" 3

# replace (sed-like)
echo "--- replace ---"
echo "hello world" | replace "world" "fshell"

# sort
echo "--- sort ---"
echo -e "banana\napple\ncherry" | sort

# uniq
echo "--- uniq ---"
echo -e "a\na\nb\nc\nc" | sort | uniq

echo ""
echo "━━━ TEXT / STRING: DONE ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 3. DATA / PARSING
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 3. DATA / PARSING ━━━"

# json (NEW builtin)
echo "--- json ---"
echo '{"name":"test","value":42}' > /tmp/test.json
cat /tmp/test.json | json
cat /tmp/test.json | json | map name value
rm /tmp/test.json

# csv (NEW builtin)
echo "--- csv ---"
printf 'name,age,city\nAlice,30,NYC\nBob,25,LA\n' | csv
printf 'name,age,city\nAlice,30,NYC\nBob,25,LA\n' | csv | filter age > 25

echo ""
echo "━━━ DATA / PARSING: DONE ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 4. GIT
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 4. GIT ━━━"

echo "--- git (native) ---"
git status | head -n 5
git log | head -n 3
git branch

echo ""
echo "━━━ GIT: DONE ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 5. PROCESS / ENVIRONMENT
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 5. PROCESS / ENVIRONMENT ━━━"

# ps (structured output)
echo "--- ps ---"
ps | head -n 3

# env (should show structured output)
echo "--- env ---"
env | head -n 3
env | filter key == "HOME"

# export / unset
echo "--- export/unset ---"
export TEST_VAR="hello"
echo $TEST_VAR
unset TEST_VAR
echo "export/unset: OK"

# set
echo "--- set ---"
set confirm_rm false
echo "set: OK"

echo ""
echo "━━━ PROCESS / ENVIRONMENT: DONE ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 6. INTERACTIVE
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 6. INTERACTIVE ━━━"

# select (NEW builtin)
echo "--- select ---"
echo "Test select manually: echo -e 'apple\nbanana\ncherry' | select"

# confirm
echo "--- confirm ---"
echo "Test confirm manually: confirm 'Delete files?'"

# ask
echo "--- ask ---"
echo "Test ask manually: ask 'What is your name?'"

# fzf (if installed)
echo "--- fzf ---"
echo "Test fzf manually: ls | fzf"

echo ""
echo "━━━ INTERACTIVE: DONE (test manually) ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 7. DISPLAY / HELP
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 7. DISPLAY / HELP ━━━"

# help
echo "--- help ---"
help

# version
echo "--- version ---"
version

# theme
echo "--- theme ---"
themes | head -n 5

echo ""
echo "━━━ DISPLAY / HELP: DONE ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 8. SYSTEM
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 8. SYSTEM ━━━"

# date
echo "--- date ---"
date

# whoami
echo "--- whoami ---"
whoami

# hostname
echo "--- hostname ---"
hostname

# uname
echo "--- uname ---"
uname

echo ""
echo "━━━ SYSTEM: DONE ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 9. SECURITY / CRYPTO
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 9. SECURITY / CRYPTO ━━━"

# hash
echo "--- hash ---"
echo "hello" | hash sha256

echo ""
echo "━━━ SECURITY / CRYPTO: DONE ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 10. CONFIG / DEV
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 10. CONFIG / DEV ━━━"

# config
echo "--- config ---"
config get theme

# reload
echo "--- reload ---"
echo "reload: OK (not testing full reload)"

echo ""
echo "━━━ CONFIG / DEV: DONE ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 11. OTHER
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 11. OTHER ━━━"

# diff (NEW builtin)
echo "--- diff ---"
echo -e "line1\nline2\nline3" > /tmp/a.txt
echo -e "line1\nline2 Modified\nline3\nline4" > /tmp/b.txt
diff /tmp/a.txt /tmp/b.txt
rm /tmp/a.txt /tmp/b.txt

# watch
echo "--- watch ---"
echo "watch: test manually with 'watch date'"

# sleep
echo "--- sleep ---"
sleep 0.1
echo "sleep: OK"

echo ""
echo "━━━ OTHER: DONE ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 12. PIPELINE OPERATORS
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 12. PIPELINE OPERATORS ━━━"

# filter
echo "--- filter ---"
ls | filter type == "file" | head -n 3
ls | filter name contains ".rs" | head -n 3

# map
echo "--- map ---"
ls | map name | head -n 3

# sort (pipeline)
echo "--- sort (pipeline) ---"
ls | sort | head -n 3

# limit
echo "--- limit ---"
ls | limit 3

# grep (pipeline)
echo "--- grep (pipeline) ---"
ls | grep ".rs" | head -n 3

echo ""
echo "━━━ PIPELINE OPERATORS: DONE ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# 13. SHELL FEATURES
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo "━━━ 13. SHELL FEATURES ━━━"

# Variable expansion
echo "--- variable expansion ---"
let x = 42
echo $x
echo "${x}"

# String interpolation
echo "--- string interpolation ---"
let name = "fshell"
echo "Hello from $name!"

# Arithmetic
echo "--- arithmetic ---"
echo $((2 + 3))

# if/else
echo "--- if/else ---"
if 1 == 1; then
    echo "if/else: OK"
fi

# match
echo "--- match ---"
match "hello" {
    "hello" => echo "match: OK"
    * => echo "match: FAIL"
}

# try/catch
echo "--- try/catch ---"
try {
    echo "try/catch: OK"
} catch e {
    echo "try/catch: FAIL"
}

# Function definition
echo "--- functions ---"
fn greet(name) { echo "Hello, $name!" }
greet "fshell"

# Alias
echo "--- aliases ---"
alias ll="ls -la"
echo "alias: OK (test manually with ll)"

# Hooks
echo "--- hooks ---"
echo "hooks: test manually with precmd/preexec"

echo ""
echo "━━━ SHELL FEATURES: DONE ━━━"
echo ""

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# SUMMARY
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

echo ""
echo "=== ALL TESTS COMPLETE ==="
echo "Review any errors above. Some tests require manual interaction:"
echo "  - select, confirm, ask (interactive prompts)"
echo "  - fzf (interactive fuzzy finder)"
echo "  - watch (continuous monitoring)"
echo "  - vault (password manager)"
echo "  - ai (AI integration)"
echo ""
