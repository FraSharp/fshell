unset baseline_value
printf '%s|%s|%s\n' "${baseline_value:-fallback}" "${baseline_value:=assigned}" "$baseline_value"
