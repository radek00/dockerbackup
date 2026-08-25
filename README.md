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
          Restore source path pointing at a dated backup directory containing backup.tar.
      --exclude-containers <excluded_containers>...
          Containers to exclude
      --exclude-volumes <excluded_volumes>...
          Volumes to exclude
  -h, --help
          Print help
```

Restore stops any running containers (except excluded ones), extracts `backup.tar` from the given source directory into the currently-existing Docker volumes with matching names, and restarts the containers afterwards.

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
