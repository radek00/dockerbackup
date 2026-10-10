# About

This is a CLI tool for backing up Docker volumes.

## Features

- Stop running containers before backup
- Restart containers after backup
- Specify multiple local or remote ssh destinations and run backups in parallel 
- Send gotify or discord notifications with backup status
- Cancel backups early with graceful shutdown
- Exclude containers and volumes from backup
- Restore volumes from a local backup
- Age encryption for backups

## Building
Binary can be obtained by running:
```bash
cargo install dockerbackup
```
or by downloading one from the available releases.

## Usage

```
Usage: dockerbackup [OPTIONS] --destination <dest_path>...

Options:
  -d, --destination <dest_path>...
          Backup destination path. This argument can be used multiple times and each path must be in the following format: [/backup or user@host:/backup].
      --exclude-containers <excluded_containers>...
          Containers to exclude from backup
      --exclude-volumes <excluded_volumes>...
          Volumes to exclude from backup
  -g, --gotify <gotify_url>
          Gotify server url for notifications
      --discord <discord_url>
          Discord webhook url for notifications
      --recipient <recipients>...
          Age recipient public key (age1...). Repeat to allow multiple decryptors.
  -h, --help
          Print help
  -V, --version
          Print version
```

### Restoring a backup

```
Restore docker volumes from a local backup

Usage: dockerbackup restore [OPTIONS] --source <source_path>

Options:
  -s, --source <source_path>
          Restore source path pointing at a dated backup directory containing backup.tar.age or backup.tar.
      --identity <identities>...
          Path to an age identity file for decrypting backup.tar.age. Repeat for key rotation/fallback.
      --exclude-containers <excluded_containers>...
          Containers to exclude
      --exclude-volumes <excluded_volumes>...
          Volumes to exclude
  -h, --help
          Print help
```

Restore stops any running containers (except excluded ones), then:
- If `backup.tar.age` exists, it is streamed through age decryption into tar extraction (requires at least one `--identity`).
- Otherwise, `backup.tar` is extracted directly.

During encrypted backups, dockerbackup streams tar output directly into age encryption and writes `backup.tar.age` to each destination, so no temporary plaintext archive is created.

Example encrypted backup:
```bash
dockerbackup \
    -d /mnt/external/backups \
    --recipient age1examplepublickey...
```

Example encrypted restore:
```bash
dockerbackup restore \
    -s /mnt/external/backups/2026-9-30 \
    --identity ~/.config/age/keys.txt
```

Notes:
- Volumes referenced in the archive that don't currently exist on the host are not automatically created — only volumes matching an existing volume name are restored into.

## Running integration tests

```bash
cd tests

# Start the test environment (dind, ssh-target, test-runner containers)
docker compose up -d --build

# Run the test scripts inside test-runner (they must run there, not on the host,
# since ssh-target is only resolvable on the compose network)
docker compose exec test-runner bash ./tests/test_script.sh
docker compose exec test-runner bash ./tests/test_edge_cases.sh
docker compose exec test-runner bash ./tests/test_container_lifecycle.sh
docker compose exec test-runner bash ./tests/test_multi_destination.sh
docker compose exec test-runner bash ./tests/test_interrupt.sh
docker compose exec test-runner bash ./tests/test_restore.sh

# Tear down afterwards
docker compose down -v --remove-orphans
```

Or simply run all of the above (including setup/teardown) with:

```bash
cd tests
./run_tests.sh
```

Test scripts:
- `test_script.sh` — full backup+restore happy path (local & SSH destinations) plus space-check failure cases.
- `test_edge_cases.sh` — nonexistent `--exclude-volumes` entry, destination directory already exists.
- `test_container_lifecycle.sh` — verifies backed-up containers are stopped and restarted, while excluded containers are left untouched.
- `test_multi_destination.sh` — multiple simultaneous destinations succeeding together, and a partial-failure scenario (one destination unreachable while others still succeed).
- `test_interrupt.sh` — Ctrl+C mid-backup, verifying temp container cleanup, container restart, and the interrupted-backup message.
- `test_restore.sh` — backup+restore round trip verifying restored file content, preserved file permissions/ownership, excluded volumes/containers left untouched, container restart, missing-archive error.
- `test_encryption.sh` — encrypted backup writes `backup.tar.age` (no plaintext `backup.tar`), encrypted restore with identity succeeds, restore without identity fails, and restore prioritizes `backup.tar.age` when both archive names exist.
