use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use crate::backup::destination::{BackupDestination, LocalDestination, SshDestination};

use super::{backup_result::BackupError, TargetOs};

pub fn check_docker() -> Result<(), BackupError> {
    let status = Command::new("docker").arg("--version").status()?;
    if status.success() {
        return Ok(());
    }
    Err(BackupError::new("Can't continue without Docker installed"))
}

pub fn check_running_containers() -> Result<String, BackupError> {
    let running_containers = Command::new("docker")
        .args(["ps", "--format", "{{.Names}}"])
        .output()?;
    let containers_list = String::from_utf8(running_containers.stdout)?;
    Ok(containers_list)
}

pub fn handle_containers(containers: &HashSet<&str>, command: &str) -> Result<(), BackupError> {
    let cmd_result = Command::new("docker")
        .arg(command)
        .args(containers)
        .status()?;
    if cmd_result.success() {
        return Ok(());
    }
    Err(BackupError::new("Error handling containers"))
}

pub fn parse_destination_path(path: &str) -> Result<Arc<dyn BackupDestination>, String> {
    if path.contains('@') {
        let tuple: Vec<&str> = path.splitn(2, ',').collect();
        if tuple.len() != 2 {
            return Err(String::from(
                "Destination path and target os must be provided",
            ));
        }

        let parts: Vec<&str> = tuple[0].splitn(2, ':').collect();
        if parts.len() == 2 && parts[0].contains('@') {
            Ok(Arc::new(SshDestination {
                host: parts[0].to_owned(),
                path: parts[1].to_owned(),
                target_os: TargetOs::from_str(tuple[1])?,
            }))
        } else {
            Err(String::from(
                "SSH path must be in the format user@host:path",
            ))
        }
    } else if Path::new(path).exists() {
        //local backups work on linux only
        Ok(Arc::new(LocalDestination {
            path: path.to_owned(),
        }))
    } else {
        Err(String::from("Local path does not exist"))
    }
}

pub fn get_volumes_size(
    volume_path: &PathBuf,
    excluded_volumes: &[String],
) -> Result<u64, BackupError> {
    let mut total_size = 0;
    let entries = fs::read_dir(volume_path)
        .map_err(|e| BackupError::new(&format!("Failed to read volume directory: {}", e)))?;

    for entry in entries {
        let entry = entry.map_err(|e| BackupError::new(&format!("Failed to read entry: {}", e)))?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if excluded_volumes.contains(&name.to_string()) {
            continue;
        }

        total_size += get_dir_size(&path).map_err(|e| {
            BackupError::new(&format!("Failed to calculate size for {}: {}", name, e))
        })?;
    }
    Ok(total_size)
}

fn get_dir_size(path: &Path) -> std::io::Result<u64> {
    let mut size = 0;
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                size += get_dir_size(&path)?;
            } else {
                size += entry.metadata()?.len();
            }
        }
    } else {
        size = path.metadata()?.len();
    }
    Ok(size)
}

pub fn get_elapsed_time(start: std::time::Instant, description: &str) -> String {
    let elapsed = start.elapsed();
    format!(
        "{}: {:02}:{:02}:{:02}",
        description,
        elapsed.as_secs() / 3600,
        elapsed.as_secs() % 3600 / 60,
        elapsed.as_secs() % 60
    )
}
