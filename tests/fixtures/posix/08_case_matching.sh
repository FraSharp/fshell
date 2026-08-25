#!/bin/sh
# EXPECT-EXIT: 0
# EXPECT-STDOUT-CONTAINS: match=fruit_apple

FRUIT="apple"
case "$FRUIT" in
    apple)
        echo "match=fruit_apple"
        ;;
    banana)
        echo "match=fruit_banana"
        ;;
    *)
        echo "match=fruit_other"
        ;;
esac
