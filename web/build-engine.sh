#!/usr/bin/env bash
# Build console-web for wasm32-unknown-emscripten and drop the single-file
# JS engine at web/engine.js. See web/BUILD.md for why each flag exists.
#
#   ./web/build-engine.sh            # release build -> web/engine.js
#   EMSDK_DIR=/opt/emsdk ./web/build-engine.sh
#
# Runnable from anywhere; paths are derived from this script's location.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(dirname -- "$script_dir")"
emsdk_dir="${EMSDK_DIR:-$HOME/emsdk}"
target=wasm32-unknown-emscripten

if [[ ! -f "$emsdk_dir/emsdk_env.sh" ]]; then
  echo "build-engine: no emsdk at $emsdk_dir (set EMSDK_DIR)" >&2
  exit 1
fi

# emsdk_env.sh is chatty and references unset vars; quarantine it.
set +u
# shellcheck disable=SC1091
source "$emsdk_dir/emsdk_env.sh" >/dev/null 2>&1
set -u

command -v emcc >/dev/null || { echo "build-engine: emcc not on PATH after sourcing emsdk_env.sh" >&2; exit 1; }

# REQUIRED: the cc crate compiles mlua's vendored Lua with plain emcc defaults,
# which emits legacy JS-longjmp calls that don't link against rustc's
# wasm-exceptions ABI (undefined symbol: emscripten_longjmp).
export CFLAGS_wasm32_unknown_emscripten="-fwasm-exceptions"

# Kept in RUSTFLAGS (not .cargo/config.toml) so native workspace builds are
# untouched by any of this.
export RUSTFLAGS="\
-C link-arg=-sSINGLE_FILE=1 \
-C link-arg=-sSINGLE_FILE_BINARY_ENCODE=0 \
-C link-arg=-sMODULARIZE=1 \
-C link-arg=-sEXPORT_NAME=ConsoleEngine \
-C link-arg=-sALLOW_MEMORY_GROWTH=1 \
-C link-arg=-sEXPORTED_FUNCTIONS=_main,_con_alloc,_con_free,_con_init,_con_init_save,_con_step,_con_width,_con_height,_con_fb,_con_audio,_con_color_count,_con_palette,_con_dpal,_con_platform_events,_con_platform_event_kind,_con_platform_event_score,_con_platform_events_dropped,_con_error,_con_save_revision,_con_save_len,_con_save,_con_save_error \
-C link-arg=-sEXPORTED_RUNTIME_METHODS=cwrap,UTF8ToString,HEAPU8"

echo "build-engine: emcc $(emcc -dumpversion), target $target"
cargo build --manifest-path "$repo_root/Cargo.toml" -p console-web --target "$target" --release

built="$repo_root/target/$target/release/console-web.js"
[[ -f "$built" ]] || { echo "build-engine: expected output missing: $built" >&2; exit 1; }

# The engine gets inlined into a <script> element by console pack; a literal
# </script anywhere in it would end the element early. Fail loud, early.
if grep -qi '</script' "$built"; then
  echo "build-engine: built engine contains '</script' — cannot be inlined" >&2
  exit 1
fi

cp "$built" "$repo_root/web/engine.js"
echo "build-engine: wrote web/engine.js ($(wc -c < "$repo_root/web/engine.js") bytes)"
