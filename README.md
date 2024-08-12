# About

This is a simple Docker backup tool. It allows you to automatically stop running containers and backup Docker volumes to local or remote destination.

## Features

- Stop running containers before backup
- Restart containers after backup
- Specify multiple local or remote ssh destinations and run backups in parallel 
- Send gotify or discord notifications with backup status
- Cancel backups early with graceful shutdown

## Building
Run the following comand to build the project:
```bash
cargo build --release
```
Binary is going to be available inside `./target/release directory`.

## Usage

```
Usage: dockerbackup [OPTIONS] --destination <dest_path>

Options:
  -d, --destination <dest_path>        Accepts multiple local or remote ssh destination paths. Destination paths must be separated by ^ and in the following format: [/backup or user@host:/backup, windows]. Target os must be specified with ssh paths.
      --volumes <volume_path>          Path to docker volumes directory [default: /var/lib/docker/volumes]
  -e, --exclude <excluded_volumes>...  Volumes to exclude from the backup
  -g, --gotify <gotify_url>            Gotify server url for notifications
      --discord <discord_url>          Discord webhook url for notifications
  -h, --help                           Print help
  -V, --version                        Print version
```
