# return unwinds out of a loop and an if at once; break and continue
# bind to the innermost loop only.

fn first_over(items, limit) {
    for item in $items {
        if $item > $limit {
            return $item
        }
    }
    return -1
}

fn sum_skipping(values, stop) {
    let total = 0
    for v in $values {
        if $v == 2 {
            continue
        }
        if $v == $stop {
            break
        }
        total = ($total + $v)
    }
    return $total
}

let xs = [1, 5, 9]
let found = first_over $xs 4
echo "found={found}"
let under = first_over $xs 100
echo "under={under}"
let skipped = sum_skipping $xs 9
echo "skipped={skipped}"

let outer = 0
for i in [1, 2, 3] {
    for j in [1, 2, 4] {
        if $j == 4 {
            break
        }
        outer = ($outer + ($i * $j))
    }
}
echo "outer={outer}"

let n = 0
while n < 10 {
    n += 1
    if $n == 3 {
        continue
    }
    if $n == 6 {
        break
    }
}
echo "n={n}"
