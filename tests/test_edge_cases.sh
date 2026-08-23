#!/bin/bash
set -e

# Setup
./tests/setup.sh

echo "Building dockerbackup..."
cargo build --release
BINARY=./target/release/dockerbackup

rm -rf /tmp/local_backup
mkdir -p /tmp/local_backup

echo "Testing --exclude-volumes with a nonexistent volume..."
if $BINARY -d /tmp/local_backup --exclude-volumes does_not_exist_vol 2>&1 | grep -q "does not exist"; then
    echo "Nonexistent excluded volume check passed."
else
    echo "Nonexistent excluded volume check failed (no error reported)!"
    exit 1
fi

echo "Testing destination directory that already exists..."
DATE_DIR=$(date +%Y-%-m-%-d)
mkdir -p "/tmp/local_backup/$DATE_DIR"

if $BINARY -d /tmp/local_backup 2>&1 | grep -q "Directory already exists"; then
    echo "Existing destination directory check passed."
else
    echo "Existing destination directory check failed (no error reported)!"
    exit 1
fi