#!/bin/bash
set -e

# Setup
./tests/setup.sh

echo "Building dockerbackup..."
cargo build --release
BINARY=./target/release/dockerbackup

rm -rf /tmp/local_backup
mkdir -p /tmp/local_backup

echo "Inflating backup_test_vol1 so the backup takes long enough to interrupt..."
docker run --rm -v backup_test_vol1:/data alpine sh -c "dd if=/dev/urandom of=/data/bigfile bs=1M count=300 2>/dev/null"

LOG=/tmp/interrupt_output.log
rm -f "$LOG"

echo "Starting backup in background..."
$BINARY \
    -d /tmp/local_backup \
    -d testuser@ssh-target:/config/remote_backup \
    --exclude-containers container_excluded \
    --exclude-volumes backup_test_vol_excluded \
    > "$LOG" 2>&1 &
BACKUP_PID=$!

sleep 2
echo "Sending SIGINT to backup process (pid $BACKUP_PID)..."
kill -INT "$BACKUP_PID"

wait "$BACKUP_PID" || true

echo "--- backup output ---"
cat "$LOG"
echo "---------------------"

if ! grep -q "Backup interrupted" "$LOG"; then
    echo "Expected interrupt message not found in output!"
    exit 1
fi

echo "Verifying temp backup containers were cleaned up..."
LEFTOVER=$(docker ps -a --format '{{.Names}}' | grep '^dockerbackup-' || true)
if [ -n "$LEFTOVER" ]; then
    echo "Leftover temp backup containers found: $LEFTOVER"
    exit 1
fi

echo "Verifying containers were restarted after interrupt..."
for name in container1 container2 container_excluded; do
    running=$(docker inspect -f '{{.State.Running}}' "$name")
    if [ "$running" != "true" ]; then
        echo "$name is not running after interrupted backup!"
        exit 1
    fi
done

echo "Interrupt handling test passed!"
