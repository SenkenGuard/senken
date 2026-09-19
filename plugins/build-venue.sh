#!/usr/bin/env bash
# Builds one venue's `wasm32-wasip2` `venue-plugin` component and copies it
# into that venue's own `dist/` (gitignored) — where its native crate's
# `include_bytes!` picks it up as the embedded, "one built-in active, no
# files required on disk" component (`AGENTS.md`; `wit/senken.wit`'s
# `venue-plugin` world).
#
# Used identically by every venue this repo ports to wasm, and by CI
# before `cargo build`/`cargo test` ever touch that venue's native crate:
# its `include_bytes!` of `dist/<venue>-venue.wasm` fails to compile until
# this script has produced that file once.
#
# `--release`, and `CARGO_TARGET_DIR` pointed at the same
# `target/fixture-wasm` every wasm test fixture in this workspace already
# shares (`crates/plugin-host/tests/support/mod.rs`), so this never grows a
# fourteenth private `target/` the way each fixture would by default.
#
# Usage:
#   plugins/build-venue.sh <venue>                          # e.g. plugins/build-venue.sh okx
#   plugins/build-venue.sh <venue> <wasm-dir> <crate-name>   # a venue with more than one market ported
#
# The two-argument form serves a venue whose one native source id was
# ported by one component (`<venue>/wasm`, producing `<venue>-venue.wasm`
# — every venue so far). The four-argument form is for a venue like OKX
# that has ported *more than one* of its native source ids, each to its
# own component under its own `<venue>/<wasm-dir>` directory (`wit/
# senken.wit`'s `venue.venue-descriptor.id` doc comment: one component
# answers for exactly one source id) — `<crate-name>` is that component's
# own Cargo package name, and its dist file is named after it rather than
# after `<venue>`, since more than one component now lives under the same
# venue directory.
set -euo pipefail

if [ $# -eq 1 ]; then
  venue="$1"
  wasm_subdir="wasm"
  crate_name="${venue}-venue"
elif [ $# -eq 3 ]; then
  venue="$1"
  wasm_subdir="$2"
  crate_name="$3"
else
  echo "usage: $0 <venue> [<wasm-dir> <crate-name>]" >&2
  exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/.." && pwd)"
venue_dir="${repo_root}/plugins/${venue}"
wasm_dir="${venue_dir}/${wasm_subdir}"

if [ ! -d "${wasm_dir}" ]; then
  echo "error: ${wasm_dir} does not exist — has ${venue} been ported to wasm32-wasip2 yet?" >&2
  exit 1
fi

target_dir="${repo_root}/target/fixture-wasm"
binary_name="$(echo "${crate_name}" | tr '-' '_').wasm"

echo "building ${crate_name} (wasm32-wasip2, release) -> ${target_dir}"
(
  cd "${wasm_dir}"
  CARGO_TARGET_DIR="${target_dir}" cargo build --release --target wasm32-wasip2
)

built="${target_dir}/wasm32-wasip2/release/${binary_name}"
if [ ! -f "${built}" ]; then
  echo "error: expected ${built} to exist after building ${crate_name}" >&2
  exit 1
fi

dist_dir="${venue_dir}/dist"
mkdir -p "${dist_dir}"
dest="${dist_dir}/${crate_name}.wasm"
cp "${built}" "${dest}"
echo "copied to ${dest}"
