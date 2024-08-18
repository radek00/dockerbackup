# About

This is a CLI tool for backing up Docker volumes.

## Features

- Stop running containers before backup
- Restart containers after backup
- Specify multiple local or remote ssh destinations and run backups in parallel 
- Send gotify or discord notifications with backup status
- Cancel backups early with graceful shutdown
- Exclude containers and volumes from backup

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
          Backup destination path. This argument can be used multiple times and each path must be in the following format: [/backup or user@host:/backup, windows]. Target os must be specified with ssh paths.
      --volumes <volume_path>
          Path to docker volumes directory [default: /var/lib/docker/volumes]
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
