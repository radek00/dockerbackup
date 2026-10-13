use backup_result::{BackupError, BackupSuccess};
use chrono::{self, Datelike};
use clap::ArgAction;
use clap::builder::styling::{AnsiColor, Effects, Styles};
use crossterm::style::Color;
use destination::{BackupDestination, SpawnedBackup};
use logger::{LogLevel, Logger};
use std::io::stdout;
use std::process::exit;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::Instant;
use utils::{
    check_docker, extract_excluded_lists, get_elapsed_time, get_volumes_size, list_backup_volumes,
    parse_destination_path, parse_source_path, with_containers_paused,
};

mod backup_result;
mod destination;
mod logger;
mod notification;
mod restore;
mod utils;

pub use restore::DockerRestore;

const BACKUP_INTERRUPTED: &str = "Backup interrupted";

type BackupChannel = (
    mpsc::Sender<Result<String, BackupError>>,
    mpsc::Receiver<Result<String, BackupError>>,
);

#[derive(Debug, Clone)]
pub struct BackupEncryptionConfig {
    pub recipients: Vec<String>,
}

pub struct DockerBackup {
    dest_paths: Vec<Arc<dyn BackupDestination>>,
    new_dir: String,
    encryption: Option<BackupEncryptionConfig>,
    excluded_containers: Vec<String>,
    excluded_volumes: Vec<String>,
    gotify_url: Option<String>,
    discord_url: Option<String>,
    receiver: Option<Receiver<Result<String, BackupError>>>,
    sender: Option<Sender<Result<String, BackupError>>>,
    logger: Arc<Logger>,
    interrupt_requested: Arc<AtomicBool>,
}

pub fn build_command() -> clap::Command {
    clap::Command::new("Docker Backup")
        .version(env!("CARGO_PKG_VERSION"))
        .author("radek00")
        .about("CLI tool for backing up docker volumes")
        .styles(Styles::styled()
        .header(AnsiColor::BrightGreen.on_default() | Effects::BOLD)
        .usage(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::Yellow.on_default()))
        .subcommand_negates_reqs(true)
        .arg(clap::Arg::new("dest_path")
            .help("Backup destination path. This argument can be used multiple times and each path must be in the following format: [/backup or user@host:/backup].")
            .required(true)
            .num_args(1..)
            .action(ArgAction::Append)
            .value_parser(parse_destination_path)
            .short('d')
        .long("destination"))
        .arg(excluded_containers_arg())
        .arg(excluded_volumes_arg())
        .arg(clap::Arg::new("gotify_url")
            .help("Gotify server url for notifications")
            .required(false)
            .short('g')
            .long("gotify"))
        .arg(clap::Arg::new("discord_url")
            .help("Discord webhook url for notifications")
            .required(false)
            .long("discord"))
        .arg(recipients_arg())
        .subcommand(
            clap::Command::new("restore")
                .about("Restore docker volumes from a local backup")
                .arg(clap::Arg::new("source_path")
                    .help("Restore source path pointing at a dated backup directory containing backup.tar.age or backup.tar.")
                    .required(true)
                    .num_args(1)
                    .value_parser(parse_source_path)
                    .short('s')
                    .long("source"))
                .arg(identities_arg())
                .arg(excluded_containers_arg())
                .arg(excluded_volumes_arg())
        )
}

fn recipients_arg() -> clap::Arg {
    clap::Arg::new("recipients")
        .help("Age recipient public key (age1...). Repeat to allow multiple decryptors.")
        .required(false)
        .long("recipient")
        .action(ArgAction::Append)
        .num_args(1..)
}

fn identities_arg() -> clap::Arg {
    clap::Arg::new("identities")
        .help("Path to an age identity file for decrypting backup.tar.age. Repeat for key rotation/fallback.")
        .required(false)
        .long("identity")
        .action(ArgAction::Append)
        .num_args(1..)
}

fn excluded_containers_arg() -> clap::Arg {
    clap::Arg::new("excluded_containers")
        .help("Containers to exclude")
        .required(false)
        .long("exclude-containers")
        .num_args(1..)
}

fn excluded_volumes_arg() -> clap::Arg {
    clap::Arg::new("excluded_volumes")
        .help("Volumes to exclude")
        .required(false)
        .long("exclude-volumes")
        .num_args(1..)
}

impl DockerBackup {
    pub fn build(mut matches: clap::ArgMatches) -> DockerBackup {
        check_docker().expect("Can't continue without Docker installed");
        let date = chrono::Local::now();
        let new_dir = format!("{}-{}-{}", date.year(), date.month(), date.day());

        let dest_paths = matches
            .remove_many::<Arc<dyn BackupDestination>>("dest_path")
            .unwrap()
            .collect();

        let recipients: Vec<String> = matches
            .remove_many::<String>("recipients")
            .map(|items| items.collect())
            .unwrap_or_default();

        let encryption = if recipients.is_empty() {
            None
        } else {
            Some(BackupEncryptionConfig { recipients })
        };

        let (excluded_containers, excluded_volumes) = extract_excluded_lists(&mut matches);

        DockerBackup {
            dest_paths,
            new_dir,
            encryption,
            excluded_containers,
            excluded_volumes,
            gotify_url: matches.remove_one::<String>("gotify_url"),
            discord_url: matches.remove_one::<String>("discord_url"),
            receiver: None,
            sender: None,
            logger: Arc::new(Logger::new(stdout())),
            interrupt_requested: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn backup(mut self) -> Result<(), BackupError> {
        self.logger.clear_terminal();

        let (sender, receiver): BackupChannel = mpsc::channel();
        let mut call_count = 0;

        let sender_clone = sender.clone();
        let logger_ctrlc = Arc::clone(&self.logger);
        let interrupt_requested = Arc::clone(&self.interrupt_requested);
        let interrupt_requested_ctrlc = Arc::clone(&interrupt_requested);
        ctrlc::set_handler(move || {
            if call_count == 0 {
                sender_clone
                    .send(Err(BackupError::new(BACKUP_INTERRUPTED)))
                    .unwrap();

                interrupt_requested_ctrlc.store(true, Ordering::Relaxed);
                call_count += 1;
            } else {
                logger_ctrlc.log("Forcing exit...", LogLevel::Warning);
                exit(1);
            }
        })
        .expect("Error setting Ctrl-C handler");

        self.receiver = Some(receiver);
        self.sender = Some(sender);

        let results = with_containers_paused(&self.excluded_containers, &self.logger, || {
            self.logger.hide_cursor();
            let results = self.run();
            self.logger.show_cursor();
            results
        })?;

        for result in results {
            match result {
                Ok(success) => {
                    success.notify(&self);
                }
                Err(err) => {
                    self.logger.log(&format!("Error: {}", err), LogLevel::Error);
                    err.notify(&self);
                }
            }
        }
        Ok(())
    }
    fn run(&self) -> Vec<Result<BackupSuccess, BackupError>> {
        self.logger.log("Backup started...", LogLevel::Info);
        let mut results: Vec<Result<BackupSuccess, BackupError>> = Vec::new();

        let volumes = match list_backup_volumes(&self.excluded_volumes) {
            Ok(volumes) => volumes,
            Err(err) => {
                results.push(Err(err));
                return results;
            }
        };

        let total_size = match get_volumes_size(&volumes) {
            Ok(size) => size,
            Err(err) => {
                results.push(Err(err));
                return results;
            }
        };

        self.logger.log(
            &format!(
                "Total size to backup: {:.2} MB",
                total_size as f64 / (1024.0 * 1024.0)
            ),
            LogLevel::Info,
        );

        let mut backup_handles: Vec<(Arc<Mutex<SpawnedBackup>>, String)> = Vec::new();

        for dest in &self.dest_paths {
            if let Err(err) = dest.check_available_space(total_size) {
                results.push(Err(err));
                continue;
            }

            if let Err(err) = dest.prepare(&self.new_dir) {
                results.push(Err(err));
                continue;
            }

            match dest.spawn_backup(&volumes, &self.new_dir, self.encryption.as_ref()) {
                Ok(spawned_backup) => {
                    backup_handles.push((
                        Arc::new(Mutex::new(spawned_backup)),
                        format!("Backup to destination {}", dest.get_display_name()),
                    ));
                }
                Err(err) => {
                    results.push(Err(err));
                }
            }
        }

        if results.len() == self.dest_paths.len() {
            return results;
        }

        let sender = self.sender.as_ref().unwrap();
        let mut join_handles: Vec<thread::JoinHandle<()>> = Vec::new();

        for (idx, handle) in backup_handles.iter().enumerate() {
            let sender_clone = sender.clone();
            let handle = handle.clone();
            let logger_clone = Arc::clone(&self.logger);
            let interrupt_flag = Arc::clone(&self.interrupt_requested);
            let join_handle = thread::spawn(move || {
                let timer = Instant::now();
                loop {
                    if interrupt_flag.load(Ordering::Relaxed) {
                        break;
                    }

                    let outcome = handle.0.lock().unwrap().poll();
                    match outcome {
                        Some(Ok(())) => {
                            let msg = get_elapsed_time(
                                timer,
                                format!("{} completed successfully in", handle.1).as_str(),
                            );
                            logger_clone.log_elapsed_time(idx, &msg, Color::Green);
                            sender_clone.send(Ok(msg)).unwrap();
                            return;
                        }
                        Some(Err(err)) => {
                            sender_clone
                                .send(Err(BackupError::new(&format!(
                                    "{} backup error: {}",
                                    handle.1, err
                                ))))
                                .unwrap();
                            return;
                        }
                        None => {
                            logger_clone.log_elapsed_time(
                                idx,
                                &get_elapsed_time(
                                    timer,
                                    format!("\r{} running time", handle.1).as_str(),
                                ),
                                Color::Cyan,
                            );
                            thread::sleep(std::time::Duration::from_secs(1));
                        }
                    }
                }
            });
            join_handles.push(join_handle);
        }

        loop {
            match self.receiver.as_ref().unwrap().try_recv() {
                Ok(message) => {
                    match message {
                        Ok(result) => results.push(Ok(BackupSuccess::new(&result))),
                        Err(err) if err.message == BACKUP_INTERRUPTED => {
                            for (backup_handle, _) in &backup_handles {
                                backup_handle.lock().unwrap().abort();
                            }
                            for join_handle in join_handles {
                                if let Err(err) = join_handle.join() {
                                    self.logger.log(
                                        &format!("Error joining thread: {:?}", err),
                                        LogLevel::Error,
                                    );
                                }
                            }
                            self.logger
                                .reset_cursor_after_timers(self.dest_paths.len() as u16);
                            self.logger.log(
                                "Backup interrupted, press Ctrl+C again to force exit",
                                LogLevel::Warning,
                            );

                            results.push(Err(BackupError::new(BACKUP_INTERRUPTED)));
                            return results;
                        }
                        Err(err) => results.push(Err(err)),
                    }

                    if results.len() == self.dest_paths.len() {
                        self.logger
                            .reset_cursor_after_timers(self.dest_paths.len() as u16);
                        self.logger.log("All backups finished", LogLevel::Success);
                        for join_handle in join_handles {
                            if let Err(err) = join_handle.join() {
                                self.logger.log(
                                    &format!("Error joining thread: {:?}", err),
                                    LogLevel::Error,
                                );
                            }
                        }

                        return results;
                    }
                }
                Err(_) => {
                    thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    }
}
