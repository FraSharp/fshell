#!/bin/sh
# EXPECT-EXIT: 0
# EXPECT-STDOUT-CONTAINS: cond=matched_true

if true; then
    echo "cond=matched_true"
else
    echo "cond=matched_false"
fi
