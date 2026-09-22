let people = [{name: "Ada", score: 8}, {name: "Ben", score: 3}]
$people | map name score | @json
