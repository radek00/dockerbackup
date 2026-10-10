#!/bin/bash
set -e

# Setup
./tests/setup.sh

echo "Building dockerbackup..."
cargo build --release
BINARY=./target/release/dockerbackup

rm -rf /tmp/encrypted_backup
mkdir -p /tmp/encrypted_backup

rm -rf /ssh_config/remote_encrypted_backup
mkdir -p /ssh_config/remote_encrypted_backup

# age crate test vector keypair (valid x25519 recipient/identity).
AGE_TEST_RECIPIENT="age1t7rxyev2z3rw82stdlrrepyc39nvn86l5078zqkf5uasdy86jp6svpy7pa"
AGE_TEST_IDENTITY_FILE="/tmp/age_identity_test.txt"
cat > "$AGE_TEST_IDENTITY_FILE" << 'EOF'
AGE-SECRET-KEY-1GQ9778VQXMMJVE8SK7J6VT8UJ4HDQAJUVSFCWCM02D8GEWQ72PVQ2Y5J33
EOF

read_file_content() {
    docker run --rm -v "$1:/data:ro" alpine cat "/data/$2"
}

echo "Running encrypted backup..."
$BINARY \
    -d /tmp/encrypted_backup \
    -d testuser@ssh-target:/config/remote_encrypted_backup \
    --recipient "$AGE_TEST_RECIPIENT" \
    --exclude-containers container_excluded \
    --exclude-volumes backup_test_vol_excluded

DATE_DIR=$(ls -1 /tmp/encrypted_backup | head -n1)
if [ -z "$DATE_DIR" ]; then
    echo "Failed to detect backup date directory under /tmp/encrypted_backup"
    exit 1
fi

echo "Verifying encrypted local output..."
if [ ! -f "/tmp/encrypted_backup/$DATE_DIR/backup.tar.age" ]; then
    echo "Encrypted local archive not found!"
    exit 1
fi

if [ -f "/tmp/encrypted_backup/$DATE_DIR/backup.tar" ]; then
    echo "Found plaintext local backup.tar for encrypted run!"
    exit 1
fi

echo "Verifying encrypted remote output..."
REMOTE_DIR="/ssh_config/remote_encrypted_backup/$DATE_DIR"
if [ ! -f "$REMOTE_DIR/backup.tar.age" ]; then
    echo "Encrypted remote archive not found at $REMOTE_DIR/backup.tar.age!"
    exit 1
fi

if [ -f "$REMOTE_DIR/backup.tar" ]; then
    echo "Found plaintext remote backup.tar for encrypted run!"
    exit 1
fi

echo "Corrupting volumes to verify encrypted restore..."
docker run --rm -v backup_test_vol1:/data alpine sh -c "echo 'CORRUPTED' > /data/file1.txt"
docker run --rm -v backup_test_vol2:/data alpine sh -c "echo 'CORRUPTED' > /data/file2.txt"

echo "Running encrypted restore with identity..."
$BINARY restore \
    -s "$REMOTE_DIR" \
    --identity "$AGE_TEST_IDENTITY_FILE" \
    --exclude-containers container_excluded \
    --exclude-volumes backup_test_vol_excluded

VOL1_CONTENT=$(read_file_content backup_test_vol1 file1.txt)
VOL2_CONTENT=$(read_file_content backup_test_vol2 file2.txt)

if [ "$VOL1_CONTENT" != "Hello World" ]; then
    echo "Encrypted restore failed for volume1: got '$VOL1_CONTENT'"
    exit 1
fi

if [ "$VOL2_CONTENT" != "Important Data" ]; then
    echo "Encrypted restore failed for volume2: got '$VOL2_CONTENT'"
    exit 1
fi

echo "Verifying restore requires identity for encrypted archive..."
if $BINARY restore -s "$REMOTE_DIR" 2>&1 | grep -q "Encrypted backup detected"; then
    echo "Identity requirement check passed."
else
    echo "Restore did not fail when identity was missing!"
    exit 1
fi

echo "Verifying encrypted archive is preferred when both files exist..."
echo "this-is-not-a-valid-tar" > "$REMOTE_DIR/backup.tar"

docker run --rm -v backup_test_vol1:/data alpine sh -c "echo 'CORRUPTED_AGAIN' > /data/file1.txt"
$BINARY restore \
    -s "$REMOTE_DIR" \
    --identity "$AGE_TEST_IDENTITY_FILE" \
    --exclude-containers container_excluded \
    --exclude-volumes backup_test_vol_excluded

VOL1_CONTENT_AFTER=$(read_file_content backup_test_vol1 file1.txt)
if [ "$VOL1_CONTENT_AFTER" != "Hello World" ]; then
    echo "Restore did not prioritize backup.tar.age over backup.tar!"
    exit 1
fi

echo "Encrypted backup/restore integration checks passed."
