#!/bin/bash
# web3d-M7: the stress scene in Twe and Three.js, measured the same way.
#
#   bash compare.sh <twe web build dir> [rounds]      (from bench/graphics/)
#
# For each GPU (Chrome's default, then the discrete one), alternates
# Three.js and Twe runs `rounds` times (default 5) and prints each side's
# median ms/frame. Alternating matters on a laptop: its GPUs throttle,
# and back-to-back runs of one side would be measured in a different
# thermal state than the other's.
set -e
twe=$1
rounds=${2:-5}
median() { printf '%s\n' "$@" | sort -n | awk '{v[NR]=$1} END {print v[int((NR+1)/2)]}'; }
for gpu in low high; do
  t=(); w=()
  for i in $(seq "$rounds"); do
    t+=($(node fps.mjs . --page three/stress.html --seconds 6 --warmup 10 --gpu $gpu | grep -o '[0-9.]* ms/frame' | cut -d' ' -f1))
    w+=($(node fps.mjs "$twe" --seconds 6 --warmup 10 --gpu $gpu | grep -o '[0-9.]* ms/frame' | cut -d' ' -f1))
  done
  echo "gpu=$gpu three: ${t[*]} -> median $(median "${t[@]}") ms"
  echo "gpu=$gpu twe:   ${w[*]} -> median $(median "${w[@]}") ms"
done
