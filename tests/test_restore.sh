#!/bin/bash
set -e

# Setup
./tests/setup.sh

echo "Building dockerbackup..."
cargo build --release
BINARY=./target/release/dockerbackup

rm -rf /tmp/restore_backup
mkdir -p /tmp/restore_backup

get_started_at() {
    docker inspect -f '{{.State.StartedAt}}' "$1"
}

read_file_content() {
    docker run --rm -v "$1:/data:ro" alpine cat "/data/$2"
}

read_file_perms() {
    docker run --rm -v "$1:/data:ro" alpine stat -c '%a %u %g' "/data/$2"
}

# Give file1.txt in backup_test_vol1 non-default ownership/permissions so we
# can verify restore preserves them exactly.
docker run --rm -v backup_test_vol1:/data alpine sh -c "chmod 640 /data/file1.txt && chown 1000:1000 /data/file1.txt"

BASELINE_CONTENT=$(read_file_content backup_test_vol1 file1.txt)
BASELINE_PERMS=$(read_file_perms backup_test_vol1 file1.txt)
echo "Baseline file1.txt: content='$BASELINE_CONTENT' perms='$BASELINE_PERMS'"

echo "Running backup..."
$BINARY \
    -d /tmp/restore_backup \
    --exclude-containers container_excluded \
    --exclude-volumes backup_test_vol_excluded

DATE_DIR=$(date +%Y-%-m-%-d)
BACKUP_DIR="/tmp/restore_backup/$DATE_DIR"

# dockerbackup and the docker daemon it talks to (dind) write the archive to
# their own view of the filesystem, which is separate from this container's
# local /tmp. The `-s` pre-flight check is a plain local fs check (same as
# the `-d` flag), so we only need a placeholder file locally for it to pass;
# the real backup.tar used during extraction is bind-mounted from dind.
touch "$BACKUP_DIR/backup.tar"

echo "Corrupting volumes to verify restore..."
docker run --rm -v backup_test_vol1:/data alpine sh -c "echo 'CORRUPTED' > /data/file1.txt && chmod 777 /data/file1.txt && chown 0:0 /data/file1.txt"
docker run --rm -v backup_test_vol2:/data alpine sh -c "echo 'CORRUPTED' > /data/file2.txt"
docker run --rm -v backup_test_vol_excluded:/data alpine sh -c "echo 'CORRUPTED' > /data/file3.txt"

CONTAINER1_BEFORE=$(get_started_at container1)
CONTAINER2_BEFORE=$(get_started_at container2)
EXCLUDED_BEFORE=$(get_started_at container_excluded)

echo "Running restore..."
$BINARY restore \
    -s "$BACKUP_DIR" \
    --exclude-containers container_excluded \
    --exclude-volumes backup_test_vol_excluded

echo "Verifying restored content..."
RESTORED_CONTENT=$(read_file_content backup_test_vol1 file1.txt)
if [ "$RESTORED_CONTENT" != "$BASELINE_CONTENT" ]; then
    echo "file1.txt content was not restored correctly! Expected '$BASELINE_CONTENT', got '$RESTORED_CONTENT'"
    exit 1
fi

echo "Verifying restored file permissions and ownership..."
RESTORED_PERMS=$(read_file_perms backup_test_vol1 file1.txt)
if [ "$RESTORED_PERMS" != "$BASELINE_PERMS" ]; then
    echo "file1.txt permissions/ownership were not preserved! Expected '$BASELINE_PERMS', got '$RESTORED_PERMS'"
    exit 1
fi

VOL2_CONTENT=$(read_file_content backup_test_vol2 file2.txt)
if [ "$VOL2_CONTENT" != "Important Data" ]; then
    echo "file2.txt was not restored correctly! Expected 'Important Data', got '$VOL2_CONTENT'"
    exit 1
fi

echo "Verifying excluded volume was left untouched..."
EXCLUDED_CONTENT=$(read_file_content backup_test_vol_excluded file3.txt)
if [ "$EXCLUDED_CONTENT" != "CORRUPTED" ]; then
    echo "Excluded volume was restored, but it should have been left untouched!"
    exit 1
fi

echo "Verifying container lifecycle..."
CONTAINER1_AFTER=$(get_started_at container1)
CONTAINER2_AFTER=$(get_started_at container2)
EXCLUDED_AFTER=$(get_started_at container_excluded)

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
        echo "$name is not running after restore!"
        exit 1
    fi
done

echo "Restore round-trip verified."

echo "Testing missing backup archive..."
mkdir -p /tmp/empty_restore_source
if $BINARY restore -s /tmp/empty_restore_source 2>&1 | grep -q "Backup archive not found"; then
    echo "Missing backup archive check passed."
else
    echo "Missing backup archive check failed (no error reported)!"
    exit 1
fi