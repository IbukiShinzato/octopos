#!/bin/bash
set -e

# add test mode marker to the filesystem
touch /tmp/testmode
./mkfs.sh /tmp/testmode

# check exit code
if ! cargo run --release; then
  echo "Test failed"
  exit 1
else
  echo "Test passed"
fi
