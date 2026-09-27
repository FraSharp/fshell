# an `if` used as a value lets return/break/continue/exit unwind, including
# when the `if` is nested inside a larger expression.

fn first_big(items, limit) {
    for x in $items {
        let skip = if $x == 0 { continue } else { 0 }
        let unused = $skip
        if $x > $limit {
            return $x
        }
    }
    return -1
}

fn nested(n) {
    let v = (if $n > 0 { return 7 } else { 0 }) + 100
    return 0
}

let ready = true
let label = if $ready { "up" } else { "down" }
echo "label={label}"

let xs = [0, 1, 5, 9]
let a = first_big $xs 4
echo "a={a}"
let ys = [1, 2, 3]
let b = first_big $ys 10
echo "b={b}"

let c = nested 1
echo "c={c}"
let d = nested 0
echo "d={d}"
