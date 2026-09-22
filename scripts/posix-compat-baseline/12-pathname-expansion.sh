: > baseline-a.glob
: > baseline-b.glob
: > baseline-c.txt
set -- baseline-*.glob
printf '<%s>\n' "$@"
