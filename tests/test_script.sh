#!/bin/bash
set -e

# Setup
./tests/setup.sh

# Build
echo "Building dockerbackup..."
cargo build --release

# Prepare destination directories
rm -rf /tmp/local_backup
mkdir -p /tmp/local_backup

BINARY=./target/release/dockerbackup

# Run backup

echo "Running backup..."
$BINARY \
    -d /tmp/local_backup \
    -d testuser@ssh-target:/config/remote_backup \
    --exclude-containers container_excluded \
    --exclude-volumes backup_test_vol_excluded

# Verify Local Backup
echo "Verifying local backup..."
DATE_DIR=$(date +%Y-%-m-%-d)
LOCAL_BACKUP_DIR="/tmp/local_backup/$DATE_DIR"

# dockerbackup talks to the dind daemon (DOCKER_HOST), so backup.tar is written
# to dind's filesystem, not test-runner's. Inspect it via a throwaway container
# against the same daemon instead of reading the path directly.
verify_local() {
    docker run --rm -v "$LOCAL_BACKUP_DIR:/verify:ro" alpine "$@"
}

if ! verify_local test -f /verify/backup.tar; then
    echo "Local backup archive not found!"
    exit 1
fi

echo "Listing local backup archive contents:"
verify_local tar -tf /verify/backup.tar

if ! verify_local tar -tf /verify/backup.tar | grep -q "^\./backup_test_vol1/file1.txt$"; then
    echo "File1 not found in local backup!"
    exit 1
fi

if verify_local tar -tf /verify/backup.tar | grep -q "backup_test_vol_excluded"; then
    echo "Excluded volume found in local backup!"
    exit 1
fi

echo "Local backup verified."

# Verify Remote Backup
echo "Verifying remote backup..."
# We can check via SSH or by looking at the shared volume /ssh_config
REMOTE_BACKUP_TAR="/ssh_config/remote_backup/$DATE_DIR/backup.tar"

if [ ! -f "$REMOTE_BACKUP_TAR" ]; then
    echo "Remote backup archive not found at $REMOTE_BACKUP_TAR!"
    exit 1
fi

echo "Listing remote backup archive contents:"
tar -tf "$REMOTE_BACKUP_TAR"

if tar -tf "$REMOTE_BACKUP_TAR" | grep -q "^\./backup_test_vol1/file1.txt$"; then
    echo "Found remote file!."
else
    echo "File1 not found in remote backup!"
    exit 1
fi

echo "Remote backup verified."

echo "Running Space Check Test..."

# 1. Local Space Check
echo "Testing Local Space Check..."
mkdir -p /tmp/small_local
mount -t tmpfs -o size=1M tmpfs /tmp/small_local
# Fill it up leaving very little space
dd if=/dev/zero of=/tmp/small_local/fill bs=1024 count=1000 2>/dev/null || true

# Run backup expecting failure
if $BINARY -d /tmp/small_local 2>&1 | grep -q "Not enough space"; then
    echo "Local space check passed (backup failed as expected)."
else
    echo "Local space check failed (backup did not fail as expected)!"
    umount /tmp/small_local
    exit 1
fi
umount /tmp/small_local

# 2. Remote Space Check
echo "Testing Remote Space Check..."
ssh -o StrictHostKeyChecking=no testuser@ssh-target "mkdir -p /config/small_remote && mount -t tmpfs -o size=1M tmpfs /config/small_remote && dd if=/dev/zero of=/config/small_remote/fill bs=1024 count=1000 2>/dev/null || true"

if $BINARY -d testuser@ssh-target:/config/small_remote 2>&1 | grep -q "Not enough space"; then
    echo "Remote space check passed (backup failed as expected)."
else
    echo "Remote space check failed (backup did not fail as expected)!"
    ssh -o StrictHostKeyChecking=no testuser@ssh-target "umount /config/small_remote"
    exit 1
fi
ssh -o StrictHostKeyChecking=no testuser@ssh-target "umount /config/small_remote"
