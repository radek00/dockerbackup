use std::{
    io::Read,
    path::Path,
    process::{Child, Command, Stdio},
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

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

pub struct SpawnedBackup {
    pub child: Child,
    pub temp_container_name: String,
    /// Present for SSH backups where docker/tar is a separate producer process.
    /// Join after `child` exits so producer failures are not dropped.
    pub producer: Option<JoinHandle<Result<(), BackupError>>>,
}

pub trait BackupDestination: std::fmt::Debug + Send + Sync {
    fn check_available_space(&self, required_size: u64) -> Result<(), BackupError> {
        let available_space = self.available_space().unwrap_or_else(|err| {
            println!(
                "Failed to check available space, but backup will be attempted anyway: {}",
                err
            );
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
            .map_err(|e| {
                BackupError::new(&format!("Failed to spawn docker backup container: {}", e))
            })?;

        Ok(SpawnedBackup {
            child,
            temp_container_name,
            producer: None,
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
                let output = ssh_command(&self.host)
                    .arg("df")
                    .arg("-B1")
                    .arg("--output=avail")
                    .arg(&self.path)
                    .output()
                    .map_err(|e| BackupError::new(&format!("Failed to execute ssh: {}", e)))?;

                if !output.status.success() {
                    return Err(BackupError::new(&format!(
                        "ssh df command failed: {}",
                        command_error_message(&output.stderr, "ssh df failed")
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
                let escaped_path = escape_for_powershell_single_quotes(&self.path);
                let script = format!(
                    "$ErrorActionPreference = 'Stop'; \
                     $path = '{escaped_path}'; \
                     if (-not (Test-Path -LiteralPath $path)) {{ throw \"Path not found: $path\" }}; \
                     $item = Get-Item -LiteralPath $path; \
                     $root = [System.IO.Path]::GetPathRoot($item.FullName); \
                     $drive = New-Object System.IO.DriveInfo $root; \
                     [Console]::Out.Write($drive.AvailableFreeSpace)"
                );
                let output = ssh_command(&self.host)
                    .arg(powershell_encoded_command(&script))
                    .output()
                    .map_err(|e| BackupError::new(&format!("Failed to execute ssh: {}", e)))?;

                if !output.status.success() {
                    return Err(BackupError::new(&format!(
                        "ssh powershell free-space command failed: {}",
                        command_error_message(&output.stderr, "ssh powershell free-space failed")
                    )));
                }

                let stdout = String::from_utf8_lossy(&output.stdout);
                stdout.trim().parse::<u64>().map_err(|_| {
                    BackupError::new(&format!(
                        "Failed to parse available space from: {}",
                        stdout.trim()
                    ))
                })
            }
        }
    }

    fn prepare(&self, new_dir: &str) -> Result<(), BackupError> {
        let dest_path = append_to_path(&self.path, new_dir, &self.target_os);

        let output = match self.target_os {
            TargetOs::Unix => {
                let escaped_dest_path = escape_for_single_quotes(&dest_path);
                ssh_command(&self.host)
                    .arg(format!("mkdir -p '{escaped_dest_path}'"))
                    .output()
                    .map_err(|e| {
                        BackupError::new(&format!("Failed to prepare remote directory: {}", e))
                    })?
            }
            TargetOs::Windows => {
                let escaped_dest_path = escape_for_powershell_single_quotes(&dest_path);
                let script = format!(
                    "$ErrorActionPreference = 'Stop'; \
                     $dir = '{escaped_dest_path}'; \
                     if (Test-Path -LiteralPath $dir) {{ throw \"Directory already exists: $dir\" }}; \
                     [void][System.IO.Directory]::CreateDirectory($dir)"
                );
                ssh_command(&self.host)
                    .arg(powershell_encoded_command(&script))
                    .output()
                    .map_err(|e| {
                        BackupError::new(&format!("Failed to prepare remote directory: {}", e))
                    })?
            }
        };

        if !output.status.success() {
            return Err(BackupError::new(&format!(
                "Failed to prepare remote directory {}: {}",
                dest_path,
                command_error_message(&output.stderr, "remote prepare failed")
            )));
        }
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
            .map_err(|e| {
                BackupError::new(&format!("Failed to spawn docker backup container: {}", e))
            })?;

        let dest_path = append_to_path(&self.path, new_dir, &self.target_os);
        let archive_path = append_to_path(&dest_path, "backup.tar", &self.target_os);

        let remote_command = match self.target_os {
            TargetOs::Unix => {
                let escaped_archive_path = escape_for_single_quotes(&archive_path);
                format!("cat > '{escaped_archive_path}'")
            }
            // Windows OpenSSH runs the remote command through cmd.exe. EncodedCommand
            // avoids cmd metacharacter parsing breaking the PowerShell script.
            TargetOs::Windows => build_windows_receive_command(&archive_path),
        };

        let docker_stdout = docker_exec.stdout.take().ok_or_else(|| {
            BackupError::new("Failed to capture backup stream from docker")
        })?;
        let docker_stderr = docker_exec.stderr.take();

        let child = ssh_command(&self.host)
            .arg(remote_command)
            .stdin(Stdio::from(docker_stdout))
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BackupError::new(&format!("Failed to spawn ssh: {}", e)))?;

        let producer = Some(thread::spawn(move || wait_for_docker_producer(docker_exec, docker_stderr)));

        Ok(SpawnedBackup {
            child,
            temp_container_name,
            producer,
        })
    }

    fn get_display_name(&self) -> String {
        format!("{}:{}", self.host, self.path)
    }
}

fn wait_for_docker_producer(
    mut docker_exec: Child,
    docker_stderr: Option<std::process::ChildStderr>,
) -> Result<(), BackupError> {
    let mut stderr_bytes = Vec::new();
    if let Some(mut stderr) = docker_stderr {
        if let Err(err) = stderr.read_to_end(&mut stderr_bytes) {
            // Still wait for the process so it is not left unreaped.
            let _ = docker_exec.wait();
            return Err(BackupError::new(&format!(
                "Failed to read docker backup stderr: {}",
                err
            )));
        }
    }

    let status = docker_exec.wait().map_err(|e| {
        BackupError::new(&format!("Failed to wait for docker backup container: {}", e))
    })?;

    if status.success() {
        return Ok(());
    }

    Err(BackupError::new(&command_error_message(
        &stderr_bytes,
        &format!("docker backup container failed with status {status}"),
    )))
}

fn ssh_command(host: &str) -> Command {
    let mut command = Command::new("ssh");
    command
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("PasswordAuthentication=no")
        .arg(host);
    command
}

fn append_to_path(path: &str, new_dir: &str, target_os: &TargetOs) -> String {
    if target_os == &TargetOs::Windows {
        let path = path.trim_end_matches(['\\', '/']);
        format!("{path}\\{new_dir}")
    } else {
        let path = path.trim_end_matches('/');
        format!("{path}/{new_dir}")
    }
}

fn escape_for_single_quotes(value: &str) -> String {
    value.replace('\'', "'\\''")
}

fn escape_for_powershell_single_quotes(value: &str) -> String {
    value.replace('\'', "''")
}

fn powershell_encoded_command(script: &str) -> String {
    // PowerShell -EncodedCommand expects UTF-16LE bytes.
    let encoded = BASE64.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    format!("powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {encoded}")
}

fn build_windows_receive_command(archive_path: &str) -> String {
    let escaped_archive_path = escape_for_powershell_single_quotes(archive_path);
    // Binary stdin -> file only. Directory creation happens in prepare().
    let script = format!(
        "$ErrorActionPreference = 'Stop'; \
         $file = '{escaped_archive_path}'; \
         $stdin = [Console]::OpenStandardInput(); \
         $out = [System.IO.File]::Create($file); \
         try {{ \
            $stdin.CopyTo($out); \
            if ($out.Length -eq 0) {{ throw 'Received empty backup stream' }} \
         }} finally {{ \
            $out.Dispose(); \
            $stdin.Dispose(); \
         }}"
    );
    powershell_encoded_command(&script)
}

fn command_error_message(stderr: &[u8], fallback: &str) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn build_temp_container_name(prefix: &str, new_dir: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("dockerbackup-{prefix}-{new_dir}-{timestamp}")
}

#[cfg(test)]
mod tests {
    use super::{build_windows_receive_command, powershell_encoded_command};

    #[test]
    fn windows_receive_command_avoids_cmd_metacharacters() {
        let command = build_windows_receive_command(r"C:\backups\backup.tar");
        assert!(command.starts_with(
            "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand "
        ));
        assert!(!command.contains('|'));
        assert!(!command.contains('<'));
        assert!(!command.contains('>'));
        assert!(!command.contains("OpenStandardInput"));
        assert!(!command.contains("backup.tar"));
    }

    #[test]
    fn encoded_command_roundtrip_prefix() {
        let command = powershell_encoded_command("Write-Output 1");
        assert!(command.contains("-EncodedCommand "));
        assert!(!command.contains("Write-Output"));
    }
}
