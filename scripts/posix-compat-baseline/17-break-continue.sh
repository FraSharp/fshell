for n in 1 2 3 4
do
    [ "$n" -eq 2 ] && continue
    [ "$n" -eq 4 ] && break
    printf '%s\n' "$n"
done
