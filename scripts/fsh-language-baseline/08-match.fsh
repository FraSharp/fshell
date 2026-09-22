let code = 404
let description = ""
match code {
    200 => { description = "OK" },
    404 => { description = "missing" },
    _ => { description = "other" }
}
echo "{description}"
