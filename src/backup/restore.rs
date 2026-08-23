use std::collections::HashSet;
use std::io::stdout;
use std::path::Path;
use std::process::{exit, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::backup::backup_result::BackupError;
use crate::backup::destination::{build_temp_container_name, SpawnedBackup};
use crate::backup::logger::{LogLevel, Logger};
use crate::backup::utils::{
    check_docker, check_running_containers, get_elapsed_time, handle_containers,
    list_backup_volumes, stop_temp_container,
};

pub struct DockerRestore {
    source_path: String,
    excluded_containers: Vec<String>,
    excluded_volumes: Vec<String>,
    logger: Arc<Logger>,
    interrupt_requested: Arc<AtomicBool>,
}

impl DockerRestore {
    pub fn build(matches: &clap::ArgMatches) -> DockerRestore {
        check_docker().expect("Can't continue without Docker installed");
        let mut matches = matches.clone();
        let excluded_containers = match matches.remove_many::<String>("excluded_containers") {
            Some(excluded_containers) => excluded_containers.collect(),
            None => Vec::new(),
        };
        let excluded_volumes = match matches.remove_many::<String>("excluded_volumes") {
            Some(excluded_volumes) => excluded_volumes.collect(),
            None => Vec::new(),
        };

        DockerRestore {
            source_path: matches.remove_one::<String>("source_path").unwrap(),
            excluded_containers,
            excluded_volumes,
            logger: Arc::new(Logger::new(stdout())),
            interrupt_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn restore(self) -> Result<(), BackupError> {
        self.logger.clear_terminal();

        let containers = check_running_containers()?;
        let mut running_containers: HashSet<&str> =
            containers.trim().split('\n').collect::<HashSet<&str>>();
        running_containers.retain(|&x| !x.is_empty());

        for container in &self.excluded_containers {
            running_containers.remove(container.as_str());
        }

        let logger_ctrlc = Arc::clone(&self.logger);
        let interrupt_requested_ctrlc = Arc::clone(&self.interrupt_requested);
        let mut call_count = 0;
        ctrlc::set_handler(move || {
            if call_count == 0 {
                interrupt_requested_ctrlc.store(true, Ordering::Relaxed);
                call_count += 1;
            } else {
                logger_ctrlc.log("Forcing exit...", LogLevel::Warning);
                exit(1);
            }
        })
        .expect("Error setting Ctrl-C handler");

        let archive_path = Path::new(&self.source_path).join("backup.tar");
        let result = if !archive_path.exists() {
            Err(BackupError::new(&format!(
                "Backup archive not found at {}",
                archive_path.display()
            )))
        } else {
            list_backup_volumes(&self.excluded_volumes)
        };

        let result = match result {
            Ok(volumes) => {
                if !running_containers.is_empty() {
                    self.logger.log("Stopping containers...", LogLevel::Info);
                    handle_containers(&running_containers, "stop")?;
                }
                let restore_result = self.run_restore(&volumes);
                if !running_containers.is_empty() {
                    self.logger.log("Starting containers...", LogLevel::Info);
                    handle_containers(&running_containers, "start")?;
                }
                restore_result
            }
            Err(err) => Err(err),
        };

        match result {
            Ok(message) => self.logger.log(&message, LogLevel::Success),
            Err(err) => self.logger.log(&format!("Error: {}", err), LogLevel::Error),
        }
        Ok(())
    }

    fn run_restore(&self, volumes: &[String]) -> Result<String, BackupError> {
        self.logger.log("Restore started...", LogLevel::Info);
        self.logger.log(
            &format!("Restoring volumes: {}", volumes.join(", ")),
            LogLevel::Info,
        );

        let spawned = spawn_restore(&self.source_path, volumes)?;
        let mut child = spawned.child;
        let timer = Instant::now();

        loop {
            if self.interrupt_requested.load(Ordering::Relaxed) {
                stop_temp_container(&spawned.temp_container_name, &self.logger);
                let _ = child.kill();
                self.logger.log(
                    "Restore interrupted, press Ctrl+C again to force exit",
                    LogLevel::Warning,
                );
                return Err(BackupError::new("Restore interrupted"));
            }

            if let Ok(Some(status)) = child.try_wait() {
                return if status.success() {
                    Ok(get_elapsed_time(
                        timer,
                        &format!(
                            "Restore from {} completed successfully in",
                            self.source_path
                        ),
                    ))
                } else {
                    Err(BackupError::new(&format!(
                        "Restore from {} failed",
                        self.source_path
                    )))
                };
            }

            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

fn spawn_restore(source_path: &str, volumes: &[String]) -> Result<SpawnedBackup, BackupError> {
    let temp_container_name = build_temp_container_name("restore", "local");

    let mut docker = Command::new("docker");
    docker
        .arg("run")
        .arg("--rm")
        .arg("--name")
        .arg(&temp_container_name);

    for volume in volumes {
        docker.arg("-v").arg(format!("{}:/data/{}", volume, volume));
    }

    let child = docker
        .arg("-v")
        .arg(format!("{}:/backup:ro", source_path))
        .arg("alpine")
        .arg("sh")
        .arg("-c")
        .arg("tar -xf /backup/backup.tar -C /data")
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            BackupError::new(&format!("Failed to spawn docker restore container: {}", e))
        })?;

    Ok(SpawnedBackup {
        child,
        temp_container_name,
    })
}
