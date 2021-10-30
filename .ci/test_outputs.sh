#!/usr/bin/env bash
set -eux

IFS=''

expected_path=$(dirname "$0")/expected
pushd $expected_path >/dev/null

out=$(cargo rtic-scope trace --resolve-only --bin example 2>/dev/null)
expected=$(cat ./example.run)

if [ $expected != $out ]; then
    exit 1
fi

popd >/dev/null
exit 0
