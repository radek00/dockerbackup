use std::{collections::HashSet, path::Path, process::Command, sync::Arc};

use crate::backup::destination::{BackupDestination, LocalDestination, SshDestination};

use super::backup_result::BackupError;

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
        let parts: Vec<&str> = path.splitn(2, ':').collect();
        if parts.len() == 2 && parts[0].contains('@') {
            Ok(Arc::new(SshDestination {
                host: parts[0].to_owned(),
                path: parts[1].to_owned(),
            }))
        } else {
            Err(String::from(
                "SSH path must be in the format user@host:path",
            ))
        }
    } else if Path::new(path).exists() {
        // local backups work on linux only
        Ok(Arc::new(LocalDestination {
            path: path.to_owned(),
        }))
    } else {
        Err(String::from("Local path does not exist"))
    }
}

pub fn list_backup_volumes(excluded_volumes: &[String]) -> Result<Vec<String>, BackupError> {
    let output = Command::new("docker")
        .args(["volume", "ls", "-q"])
        .output()
        .map_err(|e| BackupError::new(&format!("Failed to list docker volumes: {}", e)))?;

    if !output.status.success() {
        return Err(BackupError::new(&format!(
            "docker volume ls failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let all_volumes: HashSet<String> = String::from_utf8(output.stdout)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect();

    for excluded_volume in excluded_volumes {
        if !all_volumes.contains(excluded_volume) {
            return Err(BackupError::new(&format!(
                "Excluded volume '{}' does not exist",
                excluded_volume
            )));
        }
    }

    Ok(all_volumes
        .into_iter()
        .filter(|volume| !excluded_volumes.contains(volume))
        .collect())
}

pub fn get_volumes_size(included_volumes: &[String]) -> Result<u64, BackupError> {
    if included_volumes.is_empty() {
        return Ok(0);
    }

    let mut command = Command::new("docker");
    command.arg("run").arg("--rm");

    for volume in included_volumes {
        command
            .arg("-v")
            .arg(format!("{}:/data/{}:ro", volume, volume));
    }

    let output = command
        .arg("alpine")
        .arg("sh")
        .arg("-c")
        .arg("du -sk /data 2>/dev/null | cut -f1")
        .output()
        .map_err(|e| BackupError::new(&format!("Failed to calculate volumes size: {}", e)))?;

    if !output.status.success() {
        return Err(BackupError::new(&format!(
            "Volume size check failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let size_kb = String::from_utf8(output.stdout)?
        .trim()
        .parse::<u64>()
        .map_err(|_| BackupError::new("Failed to parse calculated volume size"))?;

    Ok(size_kb * 1024)
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
