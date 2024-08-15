use std::{path::Path, process::Command};

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

pub fn create_new_dir(dest_path: &Path, new_dir: &String) -> Result<(), BackupError> {
    let dir_path = dest_path.join(new_dir);
    if dir_path.exists() {
        return Err(BackupError::new("Directory already exists"));
    }
    std::fs::create_dir_all(dir_path)?;
    Ok(())
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

pub fn validate_destination_path(path: &str) -> Result<(String, TargetOs), String> {
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

pub fn parse_excluded_containers(val: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let mut excluded_containers: Vec<(String, Option<String>)> = Vec::new();
    for container in val.split(',') {
        let parts: Vec<&str> = container.splitn(2, ':').collect();
        if parts.len() == 2 {
            excluded_containers.push((parts[0].to_owned(), Some(parts[1].to_owned())));
        } else {
            excluded_containers.push((parts[0].to_owned(), None));
        }
    }
    println!("{:?}", excluded_containers);
    return Ok(excluded_containers);
}
