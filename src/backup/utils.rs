use std::{path::Path, process::Command};

use super::backup_error::BackupError;

pub fn check_docker() -> Result<(), BackupError> {
    let status = Command::new("docker").arg("--version").status()?;
    if status.success() {
        return Ok(());
    }
    Err(BackupError::new("Can't continue without Docker installed"))
}

pub fn check_running_containers() -> Result<String, BackupError> {
    let running_containers = Command::new("docker")
        .args(["container", "ls", "-q"])
        .output()?;
    let containers_list = String::from_utf8(running_containers.stdout)?;
    Ok(containers_list)
}

pub fn exclude_dirs(command: &mut Command, dirs_to_exclude: &Vec<String>) {
    for dir in dirs_to_exclude {
        command.arg(format!("--exclude={}", dir));
    }
}

pub fn create_new_dir(dest_path: &Path, new_dir: &String) -> Result<bool, BackupError> {
    let new_dir = Command::new("mkdir")
        .arg("-p")
        .arg(dest_path.join(new_dir))
        .status()?;
    Ok(new_dir.success())
}

pub fn handle_containers(containers: &Vec<&str>, command: &str) -> Result<(), BackupError> {
    let cmd_result = Command::new("docker")
        .arg(command)
        .args(containers)
        .status()?;
    if cmd_result.success() {
        return Ok(());
    }
    Err(BackupError::new("Error handling containers"))
}
