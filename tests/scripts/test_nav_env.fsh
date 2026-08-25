echo "=== Navigation & Environment ==="
pwd

mkdir -p "/tmp/fsh_test"
cd "/tmp/fsh_test"
pwd

mkdir -p subdir
pushd subdir
pwd
popd
pwd

let greeting = "hello from fsh"
echo "$greeting"

dirs

cd "/"
rm -rf "/tmp/fsh_test"
echo "=== Nav & Env OK ==="
