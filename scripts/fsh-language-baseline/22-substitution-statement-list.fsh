# a `$(...)` body is a statement list: a redirection, a command list, an
# and-or list and an assignment are accepted, and trailing newlines on the
# captured output are stripped.

echo "one=[$(echo a; echo b)]"
echo "two=[$(false || echo fallback)]"
echo "three=[$(echo hidden > /dev/null; echo shown)]"
echo "four=[$(PATH=/tmp)]"
echo "five=[$(echo hi | tr a-z A-Z)]"
