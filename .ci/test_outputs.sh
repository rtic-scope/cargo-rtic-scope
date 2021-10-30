#!/usr/bin/env bash
set -eux
IFS=''

rtic_scope=$(realpath $1)
pushd $(dirname "$0")/expected >/dev/null

# For each --bin, `trace --resolve-only` it, and compare with expected
# output.
for bin in src/bin/*.rs; do
    bin=$(basename "$bin" .rs)
    out=$($rtic_scope trace --resolve-only --bin $bin)
    expected=$(cat ./out/$bin.run)
    if [ $expected != $out ]; then
        exit 1
    fi
done

popd >/dev/null
exit 0
