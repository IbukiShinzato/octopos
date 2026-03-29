#!/bin/bash
set -e

cargo build --release --package user

# shellcheck disable=SC2046
test_bins=$(find user/testbin/*.rs | sed 's|user/testbin/\(.*\)\.rs|target/riscv64gc-unknown-none-elf/release/\1|')

# init.rs checks for this file to run testrunner instead of sh.
touch /tmp/testmode

# Pass test binaries and the testmode marker as extra files to mkfs.sh.
# shellcheck disable=SC2086
./mkfs.sh $test_bins /tmp/testmode

if ! cargo run --release; then
  echo "Test failed"
  exit 1
else
  echo "Test passed"
fi
