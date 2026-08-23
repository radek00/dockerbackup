use std::{
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::backup::backup_result::BackupError;

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
    pub child: Child,
    pub temp_container_name: String,
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
    fn spawn_backup(&self, volumes: &[String], new_dir: &str)
        -> Result<SpawnedBackup, BackupError>;
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

        let dest_path = format!("{}/{}", self.path, new_dir);
        let escaped_dest_path = escape_for_single_quotes(&dest_path);
        let remote_command = format!(
            "mkdir -p '{0}' && cat > '{0}/backup.tar'",
            escaped_dest_path
        );

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

fn escape_for_single_quotes(value: &str) -> String {
    value.replace('\'', "'\\''")
}

pub(crate) fn build_temp_container_name(prefix: &str, new_dir: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("dockerbackup-{}-{}-{}", prefix, new_dir, timestamp)
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
