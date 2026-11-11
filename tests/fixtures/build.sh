#!/usr/bin/env bash
# Rebuilds the fixture JARs from tests/fixtures/src (requires a JDK 25).
set -euo pipefail
cd "$(dirname "$0")"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

DATE=2026-01-01T00:00:00Z
JC=(javac -Xlint:-options -proc:none)

for n in 8 17 21 25; do
  mkdir -p "$work/r$n"
  "${JC[@]}" --release "$n" -d "$work/r$n" src/plain/fixture/A.java
  jar --create --date="$DATE" --file "release$n.jar" -C "$work/r$n" .
done

mkdir -p "$work/p25"
"${JC[@]}" --release 25 --enable-preview -d "$work/p25" src/preview/fixture/P.java
jar --create --date="$DATE" --file preview25.jar -C "$work/p25" .

mkdir -p "$work/out8" "$work/out9" "$work/out17"
"${JC[@]}" --release 8 -d "$work/out8" src/mr/base/fixture/M.java
"${JC[@]}" --release 9 -d "$work/out9" src/mr/module/module-info.java src/mr/base/fixture/M.java
"${JC[@]}" --release 17 -d "$work/out17" src/mr/v17/fixture/M.java
rm -rf "$work/out9/fixture"
jar --create --date="$DATE" --file multi-release.jar \
  -C "$work/out8" . \
  --release 9 -C "$work/out9" module-info.class \
  --release 17 -C "$work/out17" .
