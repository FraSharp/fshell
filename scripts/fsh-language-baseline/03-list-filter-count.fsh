let people = [{name: "Ada", score: 8}, {name: "Ben", score: 3}, {name: "Lin", score: 9}]
let passing = $people | filter score > 5 | count
echo "passing={passing}"
