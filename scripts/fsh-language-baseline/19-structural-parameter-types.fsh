fn accept_open(item: {name: String, size: Int, ..}) {
    echo "open:{item.name}:{item.size}"
}
fn accept_closed(item: {name: String, size: Int}) {
    echo "closed:{item.name}"
}
accept_open {name: "report", size: 5, ext: "txt"}
try {
    accept_closed {name: "report", size: 5, ext: "txt"}
} catch |err| {
    echo "closed-rejected-extra"
}
