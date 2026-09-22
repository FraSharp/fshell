path=src/module/file.c
printf '%s|%s\n' "${path##*/}" "${path%/*}"
