#!/bin/sh
# Run from the repo root after `cargo build --release`.
# Capture the M6 macroquad references for every 2D example.
out=tests/references/m6
log=$out/capture.log
: > $log
for e in examples/*.twe examples/*/main.twe; do
  case "$e" in */visual_fire.twe) continue;; esac
  if grep -qE "look:|camera3d|cube\(|mesh\(|sphere\(|light\.(sun|point|environment)" "$e" && [ "$e" != examples/mouse_demo.twe ]; then continue; fi
  name=$(basename "$e" .twe); [ "$name" = main ] && name=$(basename $(dirname "$e"))
  for f in 60 300; do
    png=$out/$(printf "%s-f%03d.png" "$name" $f)
    rm -f "$png"
    TWE_CAPTURE="$f:$png" timeout 60 ./target/release/twec play "$e" >/dev/null 2>$out/.err
    if [ -f "$png" ]; then echo "ok   $name f$f" >> $log; else echo "FAIL $name f$f: $(tail -2 $out/.err | tr '\n' ' ')" >> $log; fi
  done
done
rm -f $out/.err
