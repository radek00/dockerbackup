use age::{DecryptError, Decryptor, Identity, IdentityFile};
use crossterm::style::Color;
use std::fs::File;
use std::io::stdout;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{exit, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::backup::backup_result::BackupError;
use crate::backup::destination::SpawnedBackup;
use crate::backup::logger::{LogLevel, Logger};
use crate::backup::utils::{
    build_temp_container_name, check_docker, extract_excluded_lists, get_elapsed_time,
    list_backup_volumes, with_containers_paused,
};

const RESTORE_INTERRUPTED: &str = "Restore interrupted";

pub struct DockerRestore {
    source_path: String,
    identities: Vec<String>,
    excluded_containers: Vec<String>,
    excluded_volumes: Vec<String>,
    logger: Arc<Logger>,
    interrupt_requested: Arc<AtomicBool>,
}

struct ArchivePath {
    path: PathBuf,
    encrypted: bool,
}

impl DockerRestore {
    pub fn build(matches: &clap::ArgMatches) -> DockerRestore {
        check_docker().expect("Can't continue without Docker installed");
        let mut matches = matches.clone();
        let (excluded_containers, excluded_volumes) = extract_excluded_lists(&mut matches);

        DockerRestore {
            source_path: matches.remove_one::<String>("source_path").unwrap(),
            identities: matches
                .remove_many::<String>("identities")
                .map(|items| items.collect())
                .unwrap_or_default(),
            excluded_containers,
            excluded_volumes,
            logger: Arc::new(Logger::new(stdout())),
            interrupt_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn restore(self) -> Result<(), BackupError> {
        self.logger.clear_terminal();

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

        let archive = resolve_archive_path(&self.source_path)?;
        let result = if archive.encrypted && self.identities.is_empty() {
            Err(BackupError::new(
                "Encrypted backup detected. Provide at least one --identity path for restore",
            ))
        } else {
            list_backup_volumes(&self.excluded_volumes)
        };

        let result = match result {
            Ok(volumes) => with_containers_paused(&self.excluded_containers, &self.logger, || {
                self.run_restore(&volumes, &archive)
            })?,
            Err(err) => Err(err),
        };

        if let Err(err) = result {
            self.logger.log(&format!("Error: {}", err), LogLevel::Error);
        }
        Ok(())
    }

    fn run_restore(
        &self,
        volumes: &[String],
        archive: &ArchivePath,
    ) -> Result<String, BackupError> {
        self.logger.log("Restore started...", LogLevel::Info);

        let mut spawned = spawn_restore(archive, &self.identities, volumes)?;
        let timer = Instant::now();

        loop {
            if self.interrupt_requested.load(Ordering::Relaxed) {
                spawned.abort();
                self.logger.reset_cursor_after_timers(1);
                self.logger.log(
                    "Restore interrupted, press Ctrl+C again to force exit",
                    LogLevel::Warning,
                );
                return Err(BackupError::new(RESTORE_INTERRUPTED));
            }

            if let Some(outcome) = spawned.poll() {
                return match outcome {
                    Ok(()) => {
                        let msg = get_elapsed_time(
                            timer,
                            &format!(
                                "Restore from {} completed successfully in",
                                self.source_path
                            ),
                        );
                        self.logger.log_elapsed_time(0, &msg, Color::Green);
                        self.logger.reset_cursor_after_timers(1);
                        Ok(msg)
                    }
                    Err(err) => {
                        self.logger.reset_cursor_after_timers(1);
                        Err(BackupError::new(&format!(
                            "Restore from {} failed: {}",
                            self.source_path, err
                        )))
                    }
                };
            }
            self.logger.log_elapsed_time(
                0,
                get_elapsed_time(timer, "\rRestore running time").as_str(),
                Color::Cyan,
            );

            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn resolve_archive_path(source_path: &str) -> Result<ArchivePath, BackupError> {
    let encrypted_path = Path::new(source_path).join("backup.tar.age");
    if encrypted_path.exists() {
        return Ok(ArchivePath {
            path: encrypted_path,
            encrypted: true,
        });
    }

    let plain_path = Path::new(source_path).join("backup.tar");
    if plain_path.exists() {
        return Ok(ArchivePath {
            path: plain_path,
            encrypted: false,
        });
    }

    Err(BackupError::new(&format!(
        "Backup archive not found at {} or {}",
        encrypted_path.display(),
        plain_path.display()
    )))
}

fn spawn_restore(
    archive: &ArchivePath,
    identity_paths: &[String],
    volumes: &[String],
) -> Result<SpawnedBackup, BackupError> {
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

    if archive.encrypted {
        let mut child = docker
            .arg("-i")
            .arg("alpine")
            .arg("sh")
            .arg("-c")
            .arg("tar -xf - -C /data")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                BackupError::new(&format!("Failed to spawn docker restore container: {}", e))
            })?;

        let docker_stdin = child
            .stdin
            .take()
            .ok_or_else(|| BackupError::new("Failed to open docker stdin for restore"))?;
        let identity_paths = identity_paths.to_vec();
        let archive_path = archive.path.clone();

        let worker = std::thread::spawn(move || {
            let identities = load_identities(&identity_paths)?;
            let archive_file = File::open(&archive_path)?;
            decrypt_stream_to_writer(archive_file, docker_stdin, identities)
        });

        return Ok(SpawnedBackup {
            temp_container_name,
            stages: vec![("docker", child)],
            worker: Some(worker),
        });
    }

    let archive_dir = archive
        .path
        .parent()
        .ok_or_else(|| BackupError::new("Invalid archive path"))?;

    let child = docker
        .arg("-v")
        .arg(format!("{}:/backup:ro", archive_dir.display()))
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
        temp_container_name,
        stages: vec![("docker", child)],
        worker: None,
    })
}

fn load_identities(paths: &[String]) -> Result<Vec<Box<dyn Identity + Send + Sync>>, BackupError> {
    let mut identities: Vec<Box<dyn Identity + Send + Sync>> = Vec::new();

    for path in paths {
        let file_identities = IdentityFile::from_file(path.clone())
            .map_err(|e| {
                BackupError::new(&format!("Failed to read identity file {}: {}", path, e))
            })?
            .into_identities()
            .map_err(map_decrypt_error)?;

        identities.extend(file_identities);
    }

    if identities.is_empty() {
        return Err(BackupError::new(
            "No identities loaded from the provided --identity files",
        ));
    }

    Ok(identities)
}

fn decrypt_stream_to_writer<R: io::Read, W: io::Write>(
    input: R,
    output: W,
    identities: Vec<Box<dyn Identity + Send + Sync>>,
) -> Result<(), BackupError> {
    let decryptor = Decryptor::new(BufReader::new(input)).map_err(map_decrypt_error)?;
    let mut reader = decryptor
        .decrypt(
            identities
                .iter()
                .map(|identity| identity.as_ref() as &dyn Identity),
        )
        .map_err(map_decrypt_error)?;

    let mut output = BufWriter::new(output);
    io::copy(&mut reader, &mut output)?;
    output.flush()?;
    Ok(())
}

fn map_decrypt_error(err: DecryptError) -> BackupError {
    BackupError::new(&format!("Failed to decrypt age archive: {}", err))
}
