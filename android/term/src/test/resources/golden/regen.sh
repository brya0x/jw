#!/bin/sh
# Rebuilds every `.in` and its `.out` here. The expected screen of each stream comes from the
# vt100 crate the daemon runs (crates/jw/examples/cells.rs), so the emulator is held to it.
set -e
cd "$(dirname "$0")"
python3 gen.py
root=$(git rev-parse --show-toplevel)
(cd "$root" && cargo build -q -p jw --example cells)
for f in *.in; do
  base=${f%.in}
  size=${base##*.}
  rows=${size%x*}
  cols=${size#*x}
  "$root/target/debug/examples/cells" "$rows" "$cols" < "$f" > "$base.out"
done
