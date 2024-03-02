# About

This is a simple Docker backup. It allows you to automatically stop running containers and backup Docker volumes to local or remote destination.

## Features

- Stop running containers before backup
- Backup Docker volumes to a local directory or a remote server via SSH
- Restart containers after backup
- Send gotify notification about the backup status

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
  -d, --destination <dest_path>        Backup destination path. Accepts local or remote ssh path. Example: /backup or user@host:/backup
      --volumes <volume_path>          Path to docker volumes directory [default: /var/lib/docker/volumes]
  -e, --exclude <excluded_volumes>...  Volumes to exclude from the backup
  -h, --help                           Print help
  -V, --version                        Print version
```