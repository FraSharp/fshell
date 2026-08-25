#!/bin/sh
# EXPECT-EXIT: 0
# EXPECT-STDOUT-CONTAINS: prefix=world.txt
# EXPECT-STDOUT-CONTAINS: suffix=hello

STR="hello_world.txt"
echo "prefix=${STR#*_}"
echo "suffix=${STR%_*}"
