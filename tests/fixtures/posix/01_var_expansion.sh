#!/bin/sh
# EXPECT-EXIT: 0
# EXPECT-STDOUT-CONTAINS: val=custom_value
# EXPECT-STDOUT-CONTAINS: fallback=default_fallback
# EXPECT-STDOUT-CONTAINS: len=12

MY_VAR="custom_value"
echo "val=$MY_VAR"
echo "fallback=${UNSET_VAR:-default_fallback}"
echo "len=${#MY_VAR}"
