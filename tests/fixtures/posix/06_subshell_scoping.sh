#!/bin/sh
# EXPECT-EXIT: 0
# EXPECT-STDOUT-CONTAINS: inside=subshell_val
# EXPECT-STDOUT-CONTAINS: outside=outer_val

VAR="outer_val"
(
    VAR="subshell_val"
    echo "inside=$VAR"
)
echo "outside=$VAR"
