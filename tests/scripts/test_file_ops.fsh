echo "=== File Operations ==="
mkdir -p "/tmp/fsh_test"
cd "/tmp/fsh_test"

touch file_a.txt
touch file_b.txt
touch file_c.txt
ls

echo "hello world" | tee file_a.txt
echo "line two" | tee -a file_a.txt
cat file_a.txt

head -n 1 file_a.txt
tail -n 1 file_a.txt

cp file_a.txt copy_of_a.txt
ls

mv file_b.txt moved_b.txt
ls

mkdir sub
rmdir sub

rm copy_of_a.txt
ls

cd "/tmp"
rm -rf "/tmp/fsh_test"
echo "=== File Ops OK ==="
