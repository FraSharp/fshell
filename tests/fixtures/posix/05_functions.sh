#!/bin/sh
# EXPECT-EXIT: 0
# EXPECT-STDOUT-CONTAINS: hello posix world

greet() {
    echo "hello $1 $2"
}
greet "posix" "world"
