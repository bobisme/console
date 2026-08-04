#!/usr/bin/env bash
set -euo pipefail

if (($# < 1 || $# > 2)); then
  echo "usage: $0 [--extract] CART" >&2
  exit 2
fi

mode=build
if [[ ${1:-} == --extract ]]; then
  mode=extract
  shift
fi
if (($# != 1)); then
  echo "usage: $0 [--extract] CART" >&2
  exit 2
fi

cart=$1
source_dir=$(cd -- "$(dirname -- "$0")" && pwd)
source_file="$source_dir/enemy-atlas.pixels"
console_bin=${CONSOLE_BIN:-console}

names=(gnat_a gnat_b gnat_attack wasp_a wasp_b wasp_attack beetle_a beetle_b beetle_attack upper_wing_l upper_wing_r lower_wing_l lower_wing_r side_armor_l side_armor_r weak_closed weak_open claw cannon upper_pod lower_pod)
targets=(0,6,2,2 2,6,2,2 4,6,2,2 6,6,2,2 8,6,2,2 10,6,2,2 12,6,2,2 14,6,2,2 0,8,2,2 2,8,2,2 4,8,2,2 6,8,2,2 8,8,2,2 10,8,2,2 12,8,2,2 14,8,2,2 0,10,2,2 2,10,2,2 4,10,2,2 6,10,3,3 9,10,3,3)

if [[ $mode == extract ]]; then
  tmp=$(mktemp "$source_dir/.enemy-atlas.pixels.XXXXXX")
  trap 'rm -f -- "$tmp"' EXIT
  printf '%s\n' \
    '# RIBBIT RECOIL enemy atlas source' \
    '#' \
    '# Raw Apollo64 palette characters: 0-9, a-z, A-Z, -, _.' \
    '# Every frame is exact console sheet data.' \
    >>"$tmp"
  for i in "${!names[@]}"; do
    printf '\n@%s\n' "${names[$i]}" >>"$tmp"
    "$console_bin" sprite dump "$cart" "${targets[$i]}" | sed '/^#/d' >>"$tmp"
  done
  mv -- "$tmp" "$source_file"
  trap - EXIT
  exit 0
fi

for i in "${!names[@]}"; do
  name=${names[$i]}
  target=${targets[$i]}
  if ((i < 19)); then width=16; height=16; else width=24; height=24; fi
  awk -v wanted="@$name" -v width="$width" -v height="$height" '
    $0 == wanted { active=1; next }
    /^@/ && active { exit }
    active && !/^#/ && NF {
      if (length($0) != width) {
        printf "error: %s row %d has %d pixels, expected %d\n", wanted, rows+1, length($0), width > "/dev/stderr"
        exit 2
      }
      print
      rows++
    }
    END {
      if (rows != height) {
        printf "error: %s has %d rows, expected %d\n", wanted, rows, height > "/dev/stderr"
        exit 2
      }
    }
  ' "$source_file" | "$console_bin" sprite poke "$cart" "$target" --stdin
done
