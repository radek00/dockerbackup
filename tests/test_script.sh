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

echo "All tests passed!"
