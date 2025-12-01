use std::{
    collections::HashSet,
    fs,
    path::Path,
    process::{Child, Command, Stdio},
};

use crate::backup::{backup_result::BackupError, TargetOs};

#[derive(Debug, Clone)]
pub struct LocalDestination {
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct SshDestination {
    pub host: String,
    pub path: String,
    pub target_os: TargetOs,
}

pub trait BackupDestination: std::fmt::Debug + Send + Sync {
    fn check_available_space(&self, required_size: u64) -> Result<(), BackupError> {
        let available_space = self.available_space()?;

        if available_space < required_size {
            return Err(BackupError::new(&format!(
                "Not enough space on destination {}. Required: {} bytes, Available: {} bytes",
                self.get_display_name(),
                required_size,
                available_space
            )));
        }
        Ok(())
    }
    fn available_space(&self) -> Result<u64, BackupError>;

    fn prepare(&self, new_dir: &String) -> Result<(), BackupError>;
    fn spawn_backup(
        &self,
        volume_path: &Path,
        excluded_volumes: &Vec<String>,
        new_dir: &String,
    ) -> Result<Child, BackupError>;
    fn get_display_name(&self) -> String;
}

impl BackupDestination for LocalDestination {
    fn available_space(&self) -> Result<u64, BackupError> {
        let output = Command::new("df")
            .arg("-B1")
            .arg("--output=avail")
            .arg(&self.path)
            .output()
            .map_err(|e| BackupError::new(&format!("Failed to execute df: {}", e)))?;

        if !output.status.success() {
            return Err(BackupError::new(&format!(
                "df command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        if lines.len() < 2 {
            return Err(BackupError::new("Invalid df output"));
        }

        lines[1]
            .trim()
            .parse::<u64>()
            .map_err(|_| BackupError::new("Failed to parse available space"))
    }

    fn prepare(&self, new_dir: &String) -> Result<(), BackupError> {
        let dest_path = Path::new(&self.path);
        let dir_path = dest_path.join(new_dir);
        if dir_path.exists() {
            return Err(BackupError::new("Directory already exists"));
        }
        std::fs::create_dir_all(dir_path)?;
        Ok(())
    }

    fn spawn_backup(
        &self,
        volume_path: &Path,
        excluded_volumes: &Vec<String>,
        new_dir: &String,
    ) -> Result<Child, BackupError> {
        let mut rsync = Command::new("rsync");

        exclude_volumes(&mut rsync, excluded_volumes, volume_path)?;

        let exec_rsync = rsync
            .arg("-aW")
            .arg(volume_path)
            .arg(Path::new(&self.path).join(new_dir))
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BackupError::new(&format!("Failed to spawn rsync: {}", e)))?;

        Ok(exec_rsync)
    }

    fn get_display_name(&self) -> String {
        self.path.clone()
    }
}

impl BackupDestination for SshDestination {
    fn available_space(&self) -> Result<u64, BackupError> {
        match self.target_os {
            TargetOs::Unix => {
                let output = Command::new("ssh")
                    .arg(&self.host)
                    .arg("df")
                    .arg("-B1")
                    .arg("--output=avail")
                    .arg(&self.path)
                    .output()
                    .map_err(|e| BackupError::new(&format!("Failed to execute ssh: {}", e)))?;

                if !output.status.success() {
                    return Err(BackupError::new(&format!(
                        "ssh df command failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }

                let stdout = String::from_utf8_lossy(&output.stdout);
                let lines: Vec<&str> = stdout.lines().collect();
                if lines.len() < 2 {
                    return Err(BackupError::new("Invalid df output"));
                }

                lines[1]
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| BackupError::new("Failed to parse available space"))
            }
            TargetOs::Windows => {
                let ps_command = format!(
                "powershell -Command \"Get-Volume -FilePath '{}' | Select-Object -ExpandProperty SizeRemaining\"",
                self.path
            );

                let output = Command::new("ssh")
                    .arg(&self.host)
                    .arg(ps_command)
                    .output()
                    .map_err(|e| BackupError::new(&format!("Failed to execute ssh: {}", e)))?;

                if !output.status.success() {
                    return Err(BackupError::new(&format!(
                        "ssh powershell command failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }

                let stdout = String::from_utf8_lossy(&output.stdout);
                stdout
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| BackupError::new("Failed to parse available space"))
            }
        }
    }

    fn prepare(&self, _new_dir: &String) -> Result<(), BackupError> {
        Ok(())
    }

    fn spawn_backup(
        &self,
        volume_path: &Path,
        excluded_volumes: &Vec<String>,
        new_dir: &String,
    ) -> Result<Child, BackupError> {
        let mut tar_volumes = Command::new("tar");

        tar_volumes.arg("-cf-").arg("-C").arg(volume_path);

        exclude_volumes(&mut tar_volumes, excluded_volumes, volume_path)?;

        let tar_exec = tar_volumes
            .arg(".")
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| BackupError::new(&format!("Failed to spawn tar: {}", e)))?;

        let dest_path = append_to_path(&self.path, new_dir, &self.target_os);

        let ssh = Command::new("ssh")
            .arg(&self.host)
            .arg("mkdir")
            .arg(&dest_path)
            .arg("&&")
            .arg("tar")
            .arg("-C")
            .arg(dest_path)
            .arg("-xf-")
            .stdin(Stdio::from(tar_exec.stdout.unwrap()))
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BackupError::new(&format!("Failed to spawn ssh: {}", e)))?;

        Ok(ssh)
    }

    fn get_display_name(&self) -> String {
        format!("{}:{}", self.host, self.path)
    }
}

fn append_to_path(path: &str, new_dir: &String, target_os: &TargetOs) -> String {
    if target_os == &TargetOs::Windows {
        format!("{}\\{}", path, new_dir)
    } else {
        format!("{}/{}", path, new_dir)
    }
}

fn exclude_volumes(
    command: &mut Command,
    dirs_to_exclude: &Vec<String>,
    volume_path: &Path,
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
