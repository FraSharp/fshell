value=outer
(
    value=inner
    printf 'inside=%s\n' "$value"
)
printf 'outside=%s\n' "$value"
