echo "=== Pipelines ==="
mkdir -p "/tmp/fsh_test"
cd "/tmp/fsh_test"

echo apple | tee fruit.txt
echo banana | tee -a fruit.txt
echo cherry | tee -a fruit.txt
echo dragonfruit | tee -a fruit.txt
echo elderberry | tee -a fruit.txt

cat fruit.txt | count

cat fruit.txt | head -n 2

cat fruit.txt | grep a

ls "/tmp" | count

cat fruit.txt | sort

cat fruit.txt | limit 2

echo "---"
cd "/tmp"
rm -rf "/tmp/fsh_test"
echo "=== Pipelines OK ==="
