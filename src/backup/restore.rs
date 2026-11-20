use age::secrecy::SecretString;
use age::{DecryptError, Decryptor, Identity, IdentityFile, scrypt};
use crossterm::style::Color;
use std::fs::File;
use std::io::stdout;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio, exit};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::backup::backup_result::BackupError;
use crate::backup::destination::SpawnedBackup;
use crate::backup::logger::{LogLevel, Logger};
use crate::backup::utils::{
    build_temp_container_name, check_docker, extract_excluded_lists, get_elapsed_time,
    list_backup_volumes, read_passphrase, with_containers_paused,
};

const RESTORE_INTERRUPTED: &str = "Restore interrupted";

pub struct DockerRestore {
    source_path: String,
    decryption: Option<DecryptionConfig>,
    excluded_containers: Vec<String>,
    excluded_volumes: Vec<String>,
    logger: Arc<Logger>,
    interrupt_requested: Arc<AtomicBool>,
}

struct ArchivePath {
    path: PathBuf,
    encrypted: bool,
}

enum DecryptionConfig {
    Identities(Vec<String>),
    Passphrase(SecretString),
}

impl DockerRestore {
    pub fn build(matches: &clap::ArgMatches) -> DockerRestore {
        check_docker().expect("Can't continue without Docker installed");
        let mut matches = matches.clone();
        let (excluded_containers, excluded_volumes) = extract_excluded_lists(&mut matches);

        let identities = matches
            .remove_many::<String>("identities")
            .map(|items| items.collect::<Vec<_>>());
        let passphrase_path = matches.remove_one::<String>("passphrase_file");

        let decryption = match (identities, passphrase_path) {
            (Some(paths), None) => Some(DecryptionConfig::Identities(paths)),
            (None, Some(path)) => Some(DecryptionConfig::Passphrase(
                read_passphrase(&path).expect("Failed to read passphrase file"),
            )),
            (None, None) => None,
            (Some(_), Some(_)) => unreachable!("clap rejects --identity with --passphrase-file"),
        };
        DockerRestore {
            source_path: matches.remove_one::<String>("source_path").unwrap(),
            decryption,
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
        let result = match (archive.encrypted, self.decryption.as_ref()) {
            (true, None) => Err(BackupError::new(
                "Encrypted backup detected. Provide --identity or --passphrase-file for restore",
            )),
            (true, Some(config)) => check_decryption_header(&archive, config)
                .and_then(|_| list_backup_volumes(&self.excluded_volumes)),
            (false, _) => list_backup_volumes(&self.excluded_volumes),
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

        let mut spawned = spawn_restore(archive, self.decryption.as_ref(), volumes)?;
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
    decryption: Option<&DecryptionConfig>,
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
        let config = decryption.ok_or_else(|| {
            BackupError::new("Encrypted backup requires --identity or --passphrase-file")
        })?;
        let identities = build_identities(config)?;

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
        let archive_path = archive.path.clone();

        let worker = std::thread::spawn(move || {
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

fn check_decryption_header(
    archive: &ArchivePath,
    config: &DecryptionConfig,
) -> Result<(), BackupError> {
    let identities = build_identities(config)?;
    let file = File::open(&archive.path)?;
    Decryptor::new(BufReader::new(file))
        .map_err(map_decrypt_error)?
        .decrypt(
            identities
                .iter()
                .map(|identity| identity.as_ref() as &dyn Identity),
        )
        .map_err(map_decrypt_error)?;
    Ok(())
}

fn build_identities(
    config: &DecryptionConfig,
) -> Result<Vec<Box<dyn Identity + Send + Sync>>, BackupError> {
    match config {
        DecryptionConfig::Identities(paths) => load_identities(paths),
        DecryptionConfig::Passphrase(passphrase) => {
            Ok(vec![Box::new(scrypt::Identity::new(passphrase.clone()))])
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::utils::write_temp_file;

    const TEST_IDENTITY: &str =
        "AGE-SECRET-KEY-1GQ9778VQXMMJVE8SK7J6VT8UJ4HDQAJUVSFCWCM02D8GEWQ72PVQ2Y5J33";

    fn passphrase_config(passphrase: &str) -> DecryptionConfig {
        DecryptionConfig::Passphrase(SecretString::from(passphrase))
    }

    fn encrypt_with(encryptor: age::Encryptor, plaintext: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut writer = encryptor.wrap_output(&mut out).unwrap();
        writer.write_all(plaintext).unwrap();
        writer.finish().unwrap();
        out
    }

    fn encrypt_with_passphrase(passphrase: &str, plaintext: &[u8]) -> Vec<u8> {
        encrypt_with(
            age::Encryptor::with_user_passphrase(SecretString::from(passphrase)),
            plaintext,
        )
    }

    fn encrypt_to_test_recipient(plaintext: &[u8]) -> Vec<u8> {
        let recipient = TEST_IDENTITY
            .parse::<age::x25519::Identity>()
            .unwrap()
            .to_public();
        let encryptor =
            age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
                .unwrap();
        encrypt_with(encryptor, plaintext)
    }

    fn decrypt(ciphertext: &[u8], config: &DecryptionConfig) -> Result<Vec<u8>, BackupError> {
        let identities = build_identities(config)?;
        let mut out = Vec::new();
        decrypt_stream_to_writer(ciphertext, &mut out, identities)?;
        Ok(out)
    }

    fn check_header(ciphertext: &[u8], config: &DecryptionConfig) -> Result<(), BackupError> {
        let path = write_temp_file(ciphertext);
        let archive = ArchivePath {
            path: path.clone(),
            encrypted: true,
        };
        let result = check_decryption_header(&archive, config);
        std::fs::remove_file(path).unwrap();
        result
    }

    #[test]
    fn decrypts_passphrase_archive() {
        let plaintext: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let ciphertext = encrypt_with_passphrase("correct horse", &plaintext);

        let decrypted = decrypt(&ciphertext, &passphrase_config("correct horse")).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn rejects_wrong_passphrase() {
        let ciphertext = encrypt_with_passphrase("correct horse", b"data");

        let err = decrypt(&ciphertext, &passphrase_config("wrong horse")).unwrap_err();

        assert!(err.message.contains("Failed to decrypt age archive"));
    }

    #[test]
    fn decrypts_recipient_archive_with_identity_file() {
        let identity_path = write_temp_file(TEST_IDENTITY.as_bytes());
        let config =
            DecryptionConfig::Identities(vec![identity_path.to_str().unwrap().to_string()]);
        let ciphertext = encrypt_to_test_recipient(b"data");

        let decrypted = decrypt(&ciphertext, &config);
        std::fs::remove_file(identity_path).unwrap();

        assert_eq!(decrypted.unwrap(), b"data");
    }

    #[test]
    fn rejects_passphrase_archive_with_identity_file() {
        let identity_path = write_temp_file(TEST_IDENTITY.as_bytes());
        let config =
            DecryptionConfig::Identities(vec![identity_path.to_str().unwrap().to_string()]);
        let ciphertext = encrypt_with_passphrase("correct horse", b"data");

        let result = decrypt(&ciphertext, &config);
        std::fs::remove_file(identity_path).unwrap();

        assert!(result.is_err());
    }

    #[test]
    fn rejects_recipient_archive_with_passphrase() {
        let ciphertext = encrypt_to_test_recipient(b"data");

        assert!(decrypt(&ciphertext, &passphrase_config("correct horse")).is_err());
    }

    #[test]
    fn build_identities_rejects_missing_identity_file() {
        let config = DecryptionConfig::Identities(vec!["/path/that/does/not/exist".to_string()]);

        let err = build_identities(&config).err().unwrap();

        assert!(err.message.contains("Failed to read identity file"));
    }

    #[test]
    fn header_check_accepts_correct_passphrase() {
        let ciphertext = encrypt_with_passphrase("correct horse", b"data");

        assert!(check_header(&ciphertext, &passphrase_config("correct horse")).is_ok());
    }

    #[test]
    fn header_check_rejects_wrong_passphrase() {
        let ciphertext = encrypt_with_passphrase("correct horse", b"data");

        assert!(check_header(&ciphertext, &passphrase_config("wrong horse")).is_err());
    }

    #[test]
    fn header_check_rejects_non_age_file() {
        assert!(check_header(b"this-is-not-an-age-file", &passphrase_config("x")).is_err());
    }
}
