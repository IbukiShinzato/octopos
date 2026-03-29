#!/bin/bash
set -e

cargo build --release --package user

# shellcheck disable=SC2046
bins=$(find user/bin/*.rs | sed 's|user/bin/\(.*\)\.rs|target/riscv64gc-unknown-none-elf/release/\1|')

# shellcheck disable=SC2086
cargo run \
  --release \
  --manifest-path mkfs/Cargo.toml \
  --target "$(rustc -vV | grep host | cut -d' ' -f2)" -- \
  target/fs.img \
  $bins \
  LICENSE \
  "$@"
