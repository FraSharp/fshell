#!/bin/sh
# EXPECT-EXIT: 0
# EXPECT-STDOUT-CONTAINS: result=14
# EXPECT-STDOUT-CONTAINS: paren=20

X=10
RES=$((X + 2 * 2))
echo "result=$RES"
echo "paren=$(((X + 0) * 2))"
