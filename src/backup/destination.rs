use std::{
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{SystemTime, UNIX_EPOCH},
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

#[derive(Debug)]
pub struct SpawnedBackup {
    pub child: Child,
    pub temp_container_name: String,
}

pub trait BackupDestination: std::fmt::Debug + Send + Sync {
    fn check_available_space(&self, required_size: u64) -> Result<(), BackupError> {
        let available_space = self.available_space().unwrap_or_else(|err| {
            println!("Failed to check available space, but backup will be attempted anyway: {}", err);
            u64::MAX
        });

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

    fn prepare(&self, new_dir: &str) -> Result<(), BackupError>;
    fn spawn_backup(
        &self,
        volumes: &[String],
        new_dir: &str,
    ) -> Result<SpawnedBackup, BackupError>;
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

    fn prepare(&self, new_dir: &str) -> Result<(), BackupError> {
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
        volumes: &[String],
        new_dir: &str,
    ) -> Result<SpawnedBackup, BackupError> {
        let temp_container_name = build_temp_container_name("local", new_dir);
        let backup_dir = Path::new(&self.path).join(new_dir);

        let mut docker = Command::new("docker");
        docker
            .arg("run")
            .arg("--rm")
            .arg("--name")
            .arg(&temp_container_name);

        for volume in volumes {
            docker
                .arg("-v")
                .arg(format!("{}:/data/{}:ro", volume, volume));
        }

        let child = docker
            .arg("-v")
            .arg(format!("{}:/backup", backup_dir.display()))
            .arg("alpine")
            .arg("sh")
            .arg("-c")
            .arg("tar -cf /backup/backup.tar -C /data .")
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BackupError::new(&format!("Failed to spawn docker backup container: {}", e)))?;

        Ok(SpawnedBackup {
            child,
            temp_container_name,
        })
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

    fn prepare(&self, _new_dir: &str) -> Result<(), BackupError> {
        Ok(())
    }

    fn spawn_backup(
        &self,
        volumes: &[String],
        new_dir: &str,
    ) -> Result<SpawnedBackup, BackupError> {
        let temp_container_name = build_temp_container_name("ssh", new_dir);

        let mut docker = Command::new("docker");
        docker
            .arg("run")
            .arg("--rm")
            .arg("--name")
            .arg(&temp_container_name);

        for volume in volumes {
            docker
                .arg("-v")
                .arg(format!("{}:/data/{}:ro", volume, volume));
        }

        let mut docker_exec = docker
            .arg("alpine")
            .arg("sh")
            .arg("-c")
            .arg("tar -cf - -C /data .")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BackupError::new(&format!("Failed to spawn docker backup container: {}", e)))?;

        let dest_path = append_to_path(&self.path, new_dir, &self.target_os);

        let remote_command = match self.target_os {
            TargetOs::Unix => {
                let escaped_dest_path = escape_for_single_quotes(&dest_path);
                format!(
                    "mkdir -p '{0}' && cat > '{0}/backup.tar'",
                    escaped_dest_path
                )
            }
            TargetOs::Windows => {
                let escaped_dest_path = escape_for_powershell_single_quotes(&dest_path);
                let escaped_archive_path = escape_for_powershell_single_quotes(&append_to_path(
                    &dest_path,
                    "backup.tar",
                    &self.target_os,
                ));
                format!(
                    "powershell -NoProfile -Command \"$dir='{0}'; $file='{1}'; New-Item -ItemType Directory -Path $dir -Force | Out-Null; $stdin=[Console]::OpenStandardInput(); $out=[System.IO.File]::Open($file,[System.IO.FileMode]::Create,[System.IO.FileAccess]::Write,[System.IO.FileShare]::None); try {{ $stdin.CopyTo($out) }} finally {{ $out.Dispose() }}\"",
                    escaped_dest_path, escaped_archive_path
                )
            }
        };

        let docker_stdout = docker_exec
            .stdout
            .take()
            .ok_or_else(|| BackupError::new("Failed to capture backup stream from docker"))?;

        let child = Command::new("ssh")
            .arg(&self.host)
            .arg(remote_command)
            .stdin(Stdio::from(docker_stdout))
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BackupError::new(&format!("Failed to spawn ssh: {}", e)))?;

        thread::spawn(move || {
            let _ = docker_exec.wait();
        });

        Ok(SpawnedBackup {
            child,
            temp_container_name,
        })
    }

    fn get_display_name(&self) -> String {
        format!("{}:{}", self.host, self.path)
    }
}

fn append_to_path(path: &str, new_dir: &str, target_os: &TargetOs) -> String {
    if target_os == &TargetOs::Windows {
        format!("{}\\{}", path, new_dir)
    } else {
        format!("{}/{}", path, new_dir)
    }
}

fn escape_for_single_quotes(value: &str) -> String {
    value.replace('\'', "'\\''")
}

fn escape_for_powershell_single_quotes(value: &str) -> String {
    value.replace('\'', "''")
}

fn build_temp_container_name(prefix: &str, new_dir: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("dockerbackup-{}-{}-{}", prefix, new_dir, timestamp)
}
