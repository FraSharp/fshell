let shadow = "outer"
fn scoped(param) {
    local shadow = "inner"
    local temp = "local"
    echo "inside={shadow}:{param}:{temp}"
}
scoped argument
if temp == "local" {
    echo "leaked"
} else {
    echo "isolated"
}
echo "outside={shadow}"
