#!/bin/bash
set -e

# Setup
./tests/setup.sh

echo "Building dockerbackup..."
cargo build --release
BINARY=./target/release/dockerbackup

rm -rf /tmp/local_backup
mkdir -p /tmp/local_backup

get_started_at() {
    docker inspect -f '{{.State.StartedAt}}' "$1"
}

CONTAINER1_BEFORE=$(get_started_at container1)
CONTAINER2_BEFORE=$(get_started_at container2)
EXCLUDED_BEFORE=$(get_started_at container_excluded)

echo "Running backup..."
$BINARY \
    -d /tmp/local_backup \
    --exclude-containers container_excluded \
    --exclude-volumes backup_test_vol_excluded

CONTAINER1_AFTER=$(get_started_at container1)
CONTAINER2_AFTER=$(get_started_at container2)
EXCLUDED_AFTER=$(get_started_at container_excluded)

echo "Verifying container lifecycle..."

if [ "$CONTAINER1_BEFORE" == "$CONTAINER1_AFTER" ]; then
    echo "container1 was never stopped/restarted!"
    exit 1
fi

if [ "$CONTAINER2_BEFORE" == "$CONTAINER2_AFTER" ]; then
    echo "container2 was never stopped/restarted!"
    exit 1
fi

if [ "$EXCLUDED_BEFORE" != "$EXCLUDED_AFTER" ]; then
    echo "container_excluded was stopped/restarted, but it should have been left untouched!"
    exit 1
fi

for name in container1 container2 container_excluded; do
    running=$(docker inspect -f '{{.State.Running}}' "$name")
    if [ "$running" != "true" ]; then
        echo "$name is not running after backup!"
        exit 1
    fi
done

echo "Container lifecycle test passed!"
