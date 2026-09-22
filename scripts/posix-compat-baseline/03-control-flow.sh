sum=0
for n in 1 2 3 4
do
    sum=$((sum + n))
done
if [ "$sum" -eq 10 ]; then
    printf 'sum=%s\n' "$sum"
else
    exit 1
fi
