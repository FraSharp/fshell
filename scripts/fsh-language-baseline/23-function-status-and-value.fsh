# a function's status is its last command's, like the top level; a bare call
# does not print the returned value, only a captured call gets it.

fn ok() {
    true
}
fn bad() {
    false
}
fn value() {
    return 42
}

ok
echo "ok={$?}"
bad
echo "bad={$?}"

let v = value
echo "captured=[{v}]"

value
echo "bare-ok"
