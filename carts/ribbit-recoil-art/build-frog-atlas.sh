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
source_file="$source_dir/frog-atlas.pixels"
console_bin=${CONSOLE_BIN:-console}

names=(idle run_a run_b rise fall swing_tuck swing_extend laser_brace fire_breath hurt blink victory laser_eye fire_throat laser_pickup fire_pickup)
targets=(0,0,3,3 3,0,3,3 6,0,3,3 9,0,3,3 12,0,3,3 0,3,3,3 3,3,3,3 6,3,3,3 9,3,3,3 12,3,3,3 15,0,1,1 15,1,1,1 15,2,1,1 15,3,1,1 15,4,1,1 15,5,1,1)

if [[ $mode == extract ]]; then
  tmp=$(mktemp "$source_dir/.frog-atlas.pixels.XXXXXX")
  trap 'rm -f -- "$tmp"' EXIT
  printf '%s\n' \
    '# RIBBIT RECOIL frog atlas source' \
    '#' \
    '# Raw Apollo64 palette characters: 0-9, a-z, A-Z, -, _.' \
    '# Every frame is exact console sheet data; use console palette show' \
    '# for the corresponding colors.' \
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
  if ((i < 10)); then width=24; height=24; else width=8; height=8; fi
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
