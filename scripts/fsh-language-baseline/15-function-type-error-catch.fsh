fn expect_number(value: Int) {
    echo "accepted"
}
try {
    expect_number "bad"
} catch |err| {
    echo "caught"
}
