IFS=:
value=alpha:beta:gamma
set -- $value
printf '<%s>\n' "$@"
