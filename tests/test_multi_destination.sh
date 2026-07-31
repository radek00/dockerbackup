#!/bin/bash
set -e

# Setup
./tests/setup.sh

echo "Building dockerbackup..."
cargo build --release
BINARY=./target/release/dockerbackup

DATE_DIR=$(date +%Y-%-m-%-d)

verify_local() {
    docker run --rm -v "/tmp/local_backup/$DATE_DIR:/verify:ro" alpine "$@"
}

echo "Testing multiple simultaneous destinations (local + 2 remote)..."
rm -rf /tmp/local_backup
mkdir -p /tmp/local_backup
ssh -o StrictHostKeyChecking=no testuser@ssh-target "mkdir -p /config/remote_backup_a /config/remote_backup_b"

$BINARY \
    -d /tmp/local_backup \
    -d testuser@ssh-target:/config/remote_backup_a \
    -d testuser@ssh-target:/config/remote_backup_b \
    --exclude-containers container_excluded \
    --exclude-volumes backup_test_vol_excluded

if ! verify_local test -f /verify/backup.tar; then
    echo "Local destination did not produce a backup!"
    exit 1
fi

for suffix in a b; do
    TAR_PATH="/ssh_config/remote_backup_$suffix/$DATE_DIR/backup.tar"
    if [ ! -f "$TAR_PATH" ]; then
        echo "Remote backup '$suffix' not found at $TAR_PATH!"
        exit 1
    fi
done

echo "Multiple simultaneous destinations succeeded."

echo "Testing partial failure (one destination unreachable)..."
rm -rf /tmp/local_backup
mkdir -p /tmp/local_backup

OUTPUT=$($BINARY \
    -d /tmp/local_backup \
    -d testuser@unreachable-host:/config/remote_backup \
    --exclude-containers container_excluded \
    --exclude-volumes backup_test_vol_excluded 2>&1) || true

echo "$OUTPUT"

if ! echo "$OUTPUT" | grep -q "completed successfully"; then
    echo "Local destination did not report success despite the other destination failing!"
    exit 1
fi

if ! echo "$OUTPUT" | grep -qi "error"; then
    echo "Expected an error to be reported for the unreachable ssh destination!"
    exit 1
fi

if ! verify_local test -f /verify/backup.tar; then
    echo "Local backup did not complete despite the other destination failing!"
    exit 1
fi

echo "Partial failure test passed!"
echo "All multi-destination tests passed!"
