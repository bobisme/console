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
source_file="$source_dir/environment-atlas.pixels"
console_bin=${CONSOLE_BIN:-console}

names=(steel_cap girder slime_cap acid breakable steel_seam steel_left steel_right steel_damaged brace junction cavity masonry_top masonry_face masonry_corner pipe_h pipe_v pipe_elbow pipe_junction vent_grille fence_post fence_wire fence_damaged acid_lip prop_lamp prop_coil prop_crate prop_sign prop_vent prop_antenna prop_cable rust_cap rust_face concrete_cap concrete_face lab_cap lab_face pipeworks_cap pipeworks_face)
targets=(0,12,1,1 1,12,1,1 2,12,1,1 3,12,1,1 4,12,1,1 12,12,1,1 13,12,1,1 14,12,1,1 15,12,1,1 0,13,1,1 1,13,1,1 2,13,1,1 3,13,1,1 4,13,1,1 5,13,1,1 6,13,1,1 7,13,1,1 8,13,1,1 9,13,1,1 10,13,1,1 11,13,1,1 12,13,1,1 13,13,1,1 14,13,1,1 0,14,2,2 2,14,2,2 4,14,2,2 6,14,1,1 7,14,1,1 6,15,1,1 7,15,1,1 8,15,1,1 9,15,1,1 10,15,1,1 11,15,1,1 12,15,1,1 13,15,1,1 14,15,1,1 15,15,1,1)

if [[ $mode == extract ]]; then
  tmp=$(mktemp "$source_dir/.environment-atlas.pixels.XXXXXX")
  trap 'rm -f -- "$tmp"' EXIT
  printf '%s\n' \
    '# RIBBIT RECOIL environment atlas source' \
    '#' \
    '# Raw Apollo64 palette characters: 0-9, a-z, A-Z, -, _.' \
    '# Every tile and prop is exact console sheet data.' \
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
  if ((i < 24 || i >= 27)); then width=8; height=8; else width=16; height=16; fi
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
