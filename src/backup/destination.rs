use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
    process::{Child, ChildStdout, Command, ExitStatus, Stdio},
    thread::{self, JoinHandle},
};

use age::{EncryptError, Encryptor, Recipient};

use crate::backup::{
    BackupEncryptionConfig, backup_result::BackupError, utils::build_temp_container_name,
};

#[derive(Debug, Clone)]
pub struct LocalDestination {
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct SshDestination {
    pub host: String,
    pub path: String,
}

#[derive(Debug)]
pub struct SpawnedBackup {
    pub temp_container_name: String,
    pub stages: Vec<(&'static str, Child)>,
    pub worker: Option<JoinHandle<Result<(), BackupError>>>,
}

impl SpawnedBackup {
    pub fn poll(&mut self) -> Option<Result<(), BackupError>> {
        if self.worker.as_ref().is_some_and(|w| w.is_finished())
            && let Err(err) = self.join_worker()
        {
            self.abort();
            return Some(Err(err));
        }

        let mut running = false;
        let mut failures: Vec<String> = Vec::new();
        for (name, child) in &mut self.stages {
            match child.try_wait() {
                Ok(None) => running = true,
                Ok(Some(status)) if status.success() => {}
                Ok(Some(status)) => failures.push(stage_error(name, child, status)),
                Err(err) => failures.push(format!("{} status check failed: {}", name, err)),
            }
        }

        if !failures.is_empty() {
            self.abort();
            return Some(Err(BackupError::new(&failures.join("; "))));
        }
        if running || self.worker.is_some() {
            return None;
        }
        Some(Ok(()))
    }

    pub fn abort(&mut self) {
        for (_, child) in &mut self.stages {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.temp_container_name])
            .output();
        let _ = self.join_worker();
    }

    fn join_worker(&mut self) -> Result<(), BackupError> {
        match self.worker.take() {
            Some(worker) => worker
                .join()
                .unwrap_or_else(|_| Err(BackupError::new("Backup worker thread panicked"))),
            None => Ok(()),
        }
    }
}

fn stage_error(name: &str, child: &mut Child, status: ExitStatus) -> String {
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("{} exited with {}", name, status)
    } else {
        format!("{} exited with {}: {}", name, status, stderr)
    }
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

    fn prepare(&self, new_dir: &str) -> Result<(), BackupError>;
    fn spawn_backup(
        &self,
        volumes: &[String],
        new_dir: &str,
        encryption: Option<&BackupEncryptionConfig>,
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
        encryption: Option<&BackupEncryptionConfig>,
    ) -> Result<SpawnedBackup, BackupError> {
        let temp_container_name = build_temp_container_name("local", new_dir);
        let backup_dir = Path::new(&self.path).join(new_dir);

        let Some(config) = encryption else {
            let mut docker = docker_run_command(&temp_container_name, volumes);
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

            return Ok(SpawnedBackup {
                temp_container_name,
                stages: vec![("docker", child)],
                worker: None,
            });
        };

        let recipients = parse_age_recipients(&config.recipients)?;
        let output_file = File::create(backup_dir.join("backup.tar.age"))?;
        let (child, docker_stdout) = spawn_tar_stream(&temp_container_name, volumes)?;

        let worker =
            thread::spawn(move || encrypt_stream_to_writer(docker_stdout, output_file, recipients));

        Ok(SpawnedBackup {
            temp_container_name,
            stages: vec![("docker", child)],
            worker: Some(worker),
        })
    }

    fn get_display_name(&self) -> String {
        self.path.clone()
    }
}

impl BackupDestination for SshDestination {
    fn available_space(&self) -> Result<u64, BackupError> {
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

    fn prepare(&self, _new_dir: &str) -> Result<(), BackupError> {
        Ok(())
    }

    fn spawn_backup(
        &self,
        volumes: &[String],
        new_dir: &str,
        encryption: Option<&BackupEncryptionConfig>,
    ) -> Result<SpawnedBackup, BackupError> {
        let temp_container_name = build_temp_container_name("ssh", new_dir);

        let recipients = encryption
            .map(|config| parse_age_recipients(&config.recipients))
            .transpose()?;

        let dest_path = format!("{}/{}", self.path, new_dir);
        let escaped_dest_path = escape_for_single_quotes(&dest_path);
        let remote_file_name = if encryption.is_some() {
            "backup.tar.age"
        } else {
            "backup.tar"
        };
        let remote_command = format!(
            "mkdir -p '{0}' && cat > '{0}/{1}'",
            escaped_dest_path, remote_file_name
        );

        let (docker_child, docker_stdout) = spawn_tar_stream(&temp_container_name, volumes)?;
        let mut backup = SpawnedBackup {
            temp_container_name,
            stages: vec![("docker", docker_child)],
            worker: None,
        };

        match recipients {
            None => {
                let ssh = spawn_ssh(&self.host, &remote_command, Stdio::from(docker_stdout))
                    .inspect_err(|_| backup.abort())?;
                backup.stages.push(("ssh", ssh));
            }
            Some(recipients) => {
                let mut ssh = spawn_ssh(&self.host, &remote_command, Stdio::piped())
                    .inspect_err(|_| backup.abort())?;
                let ssh_stdin = ssh.stdin.take();
                backup.stages.push(("ssh", ssh));

                let Some(ssh_stdin) = ssh_stdin else {
                    backup.abort();
                    return Err(BackupError::new(
                        "Failed to open ssh stdin for backup stream",
                    ));
                };
                backup.worker = Some(thread::spawn(move || {
                    encrypt_stream_to_writer(docker_stdout, ssh_stdin, recipients)
                }));
            }
        }

        Ok(backup)
    }

    fn get_display_name(&self) -> String {
        format!("{}:{}", self.host, self.path)
    }
}

fn docker_run_command(temp_container_name: &str, volumes: &[String]) -> Command {
    let mut docker = Command::new("docker");
    docker
        .arg("run")
        .arg("--log-driver")
        .arg("none")
        .arg("--rm")
        .arg("--name")
        .arg(temp_container_name);

    for volume in volumes {
        docker
            .arg("-v")
            .arg(format!("{}:/data/{}:ro", volume, volume));
    }
    docker
}

fn spawn_tar_stream(
    temp_container_name: &str,
    volumes: &[String],
) -> Result<(Child, ChildStdout), BackupError> {
    let mut child = docker_run_command(temp_container_name, volumes)
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

    match child.stdout.take() {
        Some(stdout) => Ok((child, stdout)),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = Command::new("docker")
                .args(["rm", "-f", temp_container_name])
                .output();
            Err(BackupError::new(
                "Failed to capture backup stream from docker",
            ))
        }
    }
}

fn spawn_ssh(host: &str, remote_command: &str, stdin: Stdio) -> Result<Child, BackupError> {
    Command::new("ssh")
        .arg(host)
        .arg(remote_command)
        .stdin(stdin)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| BackupError::new(&format!("Failed to spawn ssh: {}", e)))
}

fn escape_for_single_quotes(value: &str) -> String {
    value.replace('\'', "'\\''")
}

fn parse_age_recipients(recipients: &[String]) -> Result<Vec<age::x25519::Recipient>, BackupError> {
    recipients
        .iter()
        .map(|recipient| {
            recipient.parse::<age::x25519::Recipient>().map_err(|_| {
                BackupError::new(&format!(
                    "Invalid age recipient: '{}'. Expected an age1... public key",
                    recipient
                ))
            })
        })
        .collect()
}

fn encrypt_stream_to_writer<R: io::Read, W: io::Write>(
    mut input: R,
    output: W,
    recipients: Vec<age::x25519::Recipient>,
) -> Result<(), BackupError> {
    let encryptor = Encryptor::with_recipients(recipients.iter().map(|r| r as &dyn Recipient))
        .map_err(map_encrypt_error)?;
    let mut writer = encryptor
        .wrap_output(io::BufWriter::new(output))
        .map_err(|e| BackupError::new(&format!("Failed to initialize age writer: {}", e)))?;

    io::copy(&mut input, &mut writer)?;
    let mut output = writer
        .finish()
        .map_err(|e| BackupError::new(&format!("Failed to finalize age stream: {}", e)))?;
    output.flush()?;
    Ok(())
}

fn map_encrypt_error(err: EncryptError) -> BackupError {
    BackupError::new(&format!("Failed to initialize age encryption: {}", err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_single_quotes_for_shell() {
        assert_eq!(escape_for_single_quotes("it's a test"), "it'\\''s a test");
        assert_eq!(escape_for_single_quotes("no quotes here"), "no quotes here");
    }

    #[test]
    fn builds_unique_temp_container_names() {
        let first = build_temp_container_name("local", "2026-1-1");
        let second = build_temp_container_name("local", "2026-1-1");

        assert!(first.starts_with("dockerbackup-local-2026-1-1-"));
        assert_ne!(first, second);
    }
}
