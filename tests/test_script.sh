#!/bin/bash
set -e

# Setup
./tests/setup.sh

# Build
echo "Building dockerbackup..."
cargo build

# Prepare destination directories
rm -rf /tmp/local_backup
mkdir -p /tmp/local_backup

# Run backup

echo "Running backup..."
./target/debug/dockerbackup \
    -d /tmp/local_backup \
    -d testuser@ssh-target:/config/remote_backup,unix \
    --volumes /var/lib/docker/volumes \
    --exclude-containers container_excluded \
    --exclude-volumes backup_test_vol_excluded

# Verify Local Backup
echo "Verifying local backup..."
DATE_DIR=$(date +%Y-%-m-%-d)
LOCAL_BACKUP_PATH="/tmp/local_backup/$DATE_DIR/volumes"

if [ ! -d "$LOCAL_BACKUP_PATH" ]; then
    echo "Local backup directory not found!"
    exit 1
fi

echo "Listing local backup directory:"
ls -R "$LOCAL_BACKUP_PATH"

if [ ! -f "$LOCAL_BACKUP_PATH/backup_test_vol1/_data/file1.txt" ]; then
    echo "File1 not found in local backup!"
    exit 1
fi

if [ -d "$LOCAL_BACKUP_PATH/backup_test_vol_excluded" ]; then
    echo "Excluded volume found in local backup!"
    exit 1
fi

echo "Local backup verified."

# Verify Remote Backup
echo "Verifying remote backup..."
# We can check via SSH or by looking at the shared volume /ssh_config
REMOTE_BACKUP_PATH="/ssh_config/remote_backup/$DATE_DIR"

if [ ! -d "$REMOTE_BACKUP_PATH" ]; then
    echo "Remote backup directory not found at $REMOTE_BACKUP_PATH!"
    exit 1
fi

echo "Listing remote backup directory:"
ls -R "$REMOTE_BACKUP_PATH"

 if [ -f "$REMOTE_BACKUP_PATH/backup_test_vol1/_data/file1.txt" ]; then
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
if ./target/debug/dockerbackup -d /tmp/small_local --volumes /var/lib/docker/volumes 2>&1 | grep -q "Not enough space"; then
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

if ./target/debug/dockerbackup -d testuser@ssh-target:/config/small_remote,unix --volumes /var/lib/docker/volumes 2>&1 | grep -q "Not enough space"; then
    echo "Remote space check passed (backup failed as expected)."
else
    echo "Remote space check failed (backup did not fail as expected)!"
    ssh -o StrictHostKeyChecking=no testuser@ssh-target "umount /config/small_remote"
    exit 1
fi
ssh -o StrictHostKeyChecking=no testuser@ssh-target "umount /config/small_remote"

echo "All tests passed!"
