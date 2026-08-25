#!/bin/sh
# EXPECT-EXIT: 0
# EXPECT-STDOUT-CONTAINS: total=6

sum=0
for i in 1 2 3; do
    sum=$((sum + i))
done
echo "total=$sum"
