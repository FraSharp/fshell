#!/usr/bin/env bash
# Fail release/CI builds if the archive decoder still needs build-host codecs.
set -euo pipefail
binary=${1:?usage: check-archive-linkage.sh <fsh binary>}
case "$(uname -s)" in
  Darwin)
    if otool -L "$binary" | awk 'NR > 1 {print $1}' | grep -vE '^(/usr/lib/|/System/Library/)'; then
      echo 'fsh depends on a non-system library from the build machine' >&2
      exit 1
    fi
    ;;
  Linux)
    if readelf -d "$binary" | grep NEEDED | grep -E 'lib(archive|lzma|zstd|lz4|b2)\.so'; then
      echo 'fsh depends on a non-system archive library from the build machine' >&2
      exit 1
    fi
    ;;
  *)
    echo 'archive linkage check is only supported on Unix release targets' >&2
    exit 1
    ;;
esac
