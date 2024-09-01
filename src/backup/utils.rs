use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

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

pub fn exclude_volumes(
    command: &mut Command,
    dirs_to_exclude: &Vec<String>,
    volume_path: &PathBuf,
) -> Result<(), BackupError> {
    let volumes: HashSet<String> = fs::read_dir(volume_path)
        .map_err(|e| BackupError::new(&format!("Failed to read volume directory: {}", e)))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_str().map(|s| s.to_string()))
        .collect();

    for volume in dirs_to_exclude {
        if !volumes.iter().any(|x| x.ends_with(volume)) {
            return Err(BackupError::new(&format!(
                "Excluded volume '{}' does not exist",
                volume
            )));
        }
        command.arg(format!("--exclude={}", volume));
    }
    Ok(())
}

pub fn create_new_dir(dest_path: &Path, new_dir: &String) -> Result<(), BackupError> {
    let dir_path = dest_path.join(new_dir);
    if dir_path.exists() {
        return Err(BackupError::new("Directory already exists"));
    }
    std::fs::create_dir_all(dir_path)?;
    Ok(())
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

pub fn parse_destination_path(path: &str) -> Result<(String, TargetOs), String> {
    if path.contains('@') {
        let tuple: Vec<&str> = path.splitn(2, ',').collect();
        if tuple.len() != 2 {
            return Err(String::from(
                "Destination path and target os must be provided",
            ));
        }

        let parts: Vec<&str> = tuple[0].splitn(2, ':').collect();
        if parts.len() == 2 && parts[0].contains('@') {
            Ok((tuple[0].to_owned(), TargetOs::from_str(tuple[1])?))
        } else {
            Err(String::from(
                "SSH path must be in the format user@host:path",
            ))
        }
    } else if Path::new(path).exists() {
        Ok((path.to_owned(), TargetOs::Windows))
    } else {
        Err(String::from("Local path does not exist"))
    }
}

pub fn get_elapsed_time(start: std::time::Instant) -> String {
    let elapsed = start.elapsed();
    format!(
        "{:02}:{:02}:{:02}",
        elapsed.as_secs() / 3600,
        elapsed.as_secs() % 3600 / 60,
        elapsed.as_secs() % 60
    )
}
