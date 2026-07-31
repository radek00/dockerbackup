use backup_result::{BackupError, BackupSuccess};
use chrono::{self, Datelike};
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::ArgAction;
use crossterm::style::Color;
use std::collections::HashSet;
use std::io::{stdout, BufReader, Read};
use std::process::{exit, Child};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;
use utils::{
    check_docker, check_running_containers, get_elapsed_time, get_volumes_size, handle_containers,
    list_backup_volumes, parse_destination_path,
};

use crate::backup::destination::BackupDestination;
use crate::backup::logger::{LogLevel, Logger};

mod backup_result;
mod destination;
mod logger;
mod notification;
mod utils;

type BackupChannel = (
    mpsc::Sender<Result<String, BackupError>>,
    mpsc::Receiver<Result<String, BackupError>>,
);

type BackupHandle = (
    Arc<Mutex<Child>>,
    String,
    Option<JoinHandle<Result<(), BackupError>>>,
);

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TargetOs {
    Unix,
    Windows,
}

impl TargetOs {
    fn from_str(os: &str) -> Result<Self, String> {
        let os = os.to_lowercase();
        if os == "windows" {
            return Ok(TargetOs::Windows);
        } else if os == "unix" {
            return Ok(TargetOs::Unix);
        }
        Err(String::from("Unsupported os"))
    }
}

pub struct DockerBackup {
    dest_paths: Vec<Arc<dyn BackupDestination>>,
    new_dir: String,
    excluded_containers: Vec<String>,
    excluded_volumes: Vec<String>,
    gotify_url: Option<String>,
    discord_url: Option<String>,
    receiver: Option<Receiver<Result<String, BackupError>>>,
    sender: Option<Sender<Result<String, BackupError>>>,
    logger: Arc<Logger>,
    temp_containers: Arc<Mutex<HashSet<String>>>,
}

impl DockerBackup {
    pub fn build() -> DockerBackup {
        check_docker().expect("Can't continue without Docker installed");
        let date = chrono::Local::now();
        let new_dir = format!("{}-{}-{}", date.year(), date.month(), date.day());

        let mut matches = clap::Command::new("Docker Backup")
            .version(env!("CARGO_PKG_VERSION"))
            .author("radek00")
            .about("CLI tool for backing up docker volumes")
            .styles(Styles::styled()
            .header(AnsiColor::BrightGreen.on_default() | Effects::BOLD)
            .usage(AnsiColor::Yellow.on_default() | Effects::BOLD)
            .placeholder(AnsiColor::Yellow.on_default()))
            .arg(clap::Arg::new("dest_path")
                .help("Backup destination path. This argument can be used multiple times and each path must be in the following format: [/backup or user@host:/backup, windows]. Target os must be specified with ssh paths.")
                .required(true)
                .num_args(1..)
                .action(ArgAction::Append)
                .value_parser(parse_destination_path)
                .short('d')
            .long("destination"))
            .arg(clap::Arg::new("excluded_containers")
                .help("Containers to exclude from backup")
                .required(false)
                .long("exclude-containers")
                .num_args(1..))
            .arg(clap::Arg::new("excluded_volumes")
                .help("Volumes to exclude from backup")
                .required(false)
                .long("exclude-volumes")
                .num_args(1..))
            .arg(clap::Arg::new("gotify_url")
                .help("Gotify server url for notifications")
                .required(false)
                .short('g')
                .long("gotify"))
            .arg(clap::Arg::new("discord_url")
                .help("Discord webhook url for notifications")
                .required(false)
                .long("discord"))
            .get_matches();

        let excluded_containers = match matches.remove_many::<String>("excluded_containers") {
            Some(excluded_containers) => excluded_containers.collect(),
            None => Vec::new(),
        };
        let mut excluded_volumes = match matches.remove_many::<String>("excluded_volumes") {
            Some(excluded_volumes) => excluded_volumes.collect(),
            None => Vec::new(),
        };

        DockerBackup {
            dest_paths: matches
                .remove_many::<Arc<dyn BackupDestination>>("dest_path")
                .unwrap()
                .collect(),
            new_dir,
            excluded_containers,
            excluded_volumes,
            gotify_url: matches.remove_one::<String>("gotify_url"),
            discord_url: matches.remove_one::<String>("discord_url"),
            receiver: None,
            sender: None,
            logger: Arc::new(Logger::new(stdout())),
            temp_containers: Arc::new(Mutex::new(HashSet::new())),
        }
    }
    pub fn backup(mut self) -> Result<(), BackupError> {
        self.logger.clear_terminal();
        let containers = check_running_containers()?;
        let mut running_containers: HashSet<&str> =
            containers.trim().split('\n').collect::<HashSet<&str>>();
        running_containers.retain(|&x| !x.is_empty());

        for container in &self.excluded_containers {
            running_containers.remove(container.as_str());
        }

        let (sender, receiver): BackupChannel = mpsc::channel();
        let mut call_count = 0;

        let sender_clone = sender.clone();
        let logger_ctrlc = Arc::clone(&self.logger);
        let temp_containers_ctrlc = Arc::clone(&self.temp_containers);
        ctrlc::set_handler(move || {
            if call_count == 0 {
                sender_clone
                    .send(Err(BackupError::new("Backup interrupted")))
                    .unwrap();

                call_count += 1;
            } else {
                cleanup_temp_containers(&temp_containers_ctrlc, &logger_ctrlc);
                logger_ctrlc.log("Forcing exit...", LogLevel::Warning);
                exit(1);
            }
        })
        .expect("Error setting Ctrl-C handler");

        self.receiver = Some(receiver);
        self.sender = Some(sender);

        if !running_containers.is_empty() {
            self.logger.log("Stopping containers...", LogLevel::Info);
            handle_containers(&running_containers, "stop")?;
        }

        self.logger.hide_cursor();
        let results = self.run();
        self.logger.show_cursor();

        if !running_containers.is_empty() {
            self.logger.log("Starting containers...", LogLevel::Info);
            handle_containers(&running_containers, "start")?;
        }

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

        // (child, label, optional docker/tar producer join handle)
        let mut backup_handles: Vec<BackupHandle> = Vec::new();

        for dest in &self.dest_paths {
            if let Err(err) = dest.check_available_space(total_size) {
                results.push(Err(err));
                continue;
            }

            if let Err(err) = dest.prepare(&self.new_dir) {
                results.push(Err(err));
                continue;
            }

            match dest.spawn_backup(&volumes, &self.new_dir) {
                Ok(spawned_backup) => {
                    if let Ok(mut container_set) = self.temp_containers.lock() {
                        container_set.insert(spawned_backup.temp_container_name);
                    }
                    backup_handles.push((
                        Arc::new(Mutex::new(spawned_backup.child)),
                        format!("Backup to destination {}", dest.get_display_name()),
                        spawned_backup.producer,
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
        // Keep child Arcs so Ctrl+C can still kill in-flight transfer processes.
        let mut kill_handles: Vec<Arc<Mutex<Child>>> = Vec::new();

        for (idx, (child, label, producer)) in backup_handles.into_iter().enumerate() {
            kill_handles.push(Arc::clone(&child));
            let sender_clone = sender.clone();
            let logger_clone = Arc::clone(&self.logger);
            let join_handle = thread::spawn(move || {
                let timer = Instant::now();
                let stderr = child.lock().unwrap().stderr.take();
                let mut stderr_reader = stderr.map(BufReader::new);
                let mut buffer = Vec::new();
                let transfer_result = loop {
                    match child.lock().unwrap().try_wait() {
                        Ok(Some(status)) => {
                            if status.success() {
                                break Ok(());
                            }

                            let detail = if let Some(reader) = stderr_reader.as_mut() {
                                match reader.read_to_end(&mut buffer) {
                                    Ok(_) => {
                                        let stderr_output = String::from_utf8_lossy(&buffer);
                                        let trimmed = stderr_output.trim();
                                        if trimmed.is_empty() {
                                            format!("{} failed with status {}", label, status)
                                        } else {
                                            trimmed.to_string()
                                        }
                                    }
                                    Err(e) => {
                                        logger_clone.log(
                                            &format!("Failed to read stderr: {}", e),
                                            LogLevel::Error,
                                        );
                                        format!(
                                            "{} failed with status {} (also failed reading stderr: {})",
                                            label, status, e
                                        )
                                    }
                                }
                            } else {
                                format!("{} failed with status {}", label, status)
                            };
                            break Err(BackupError::new(&detail));
                        }
                        Ok(None) => {
                            logger_clone.log_elapsed_time(
                                idx,
                                &get_elapsed_time(
                                    timer,
                                    format!("\r{} running time", label).as_str(),
                                ),
                                Color::Cyan,
                            );
                            thread::sleep(std::time::Duration::from_secs(1));
                        }
                        Err(e) => {
                            break Err(BackupError::new(&format!(
                                "Failed to wait for {}: {}",
                                label, e
                            )));
                        }
                    }
                };

                // Join docker/tar producer after the transfer child exits so its
                // failure is never discarded (e.g. tar dies while ssh still exits 0).
                let producer_result = match producer {
                    Some(handle) => match handle.join() {
                        Ok(result) => result,
                        Err(_) => Err(BackupError::new(&format!(
                            "{}: backup producer thread panicked",
                            label
                        ))),
                    },
                    None => Ok(()),
                };

                match (transfer_result, producer_result) {
                    (Ok(()), Ok(())) => {
                        let msg = get_elapsed_time(
                            timer,
                            format!("{} completed successfully in", label).as_str(),
                        );
                        logger_clone.log_elapsed_time(idx, &msg, Color::Green);
                        sender_clone.send(Ok(msg)).unwrap();
                    }
                    (Err(transfer_err), Ok(())) => {
                        sender_clone.send(Err(transfer_err)).unwrap();
                    }
                    (Ok(()), Err(producer_err)) => {
                        sender_clone
                            .send(Err(BackupError::new(&format!(
                                "{}: {}",
                                label, producer_err
                            ))))
                            .unwrap();
                    }
                    (Err(transfer_err), Err(producer_err)) => {
                        sender_clone
                            .send(Err(BackupError::new(&format!(
                                "{}; producer: {}",
                                transfer_err, producer_err
                            ))))
                            .unwrap();
                    }
                }
            });
            join_handles.push(join_handle);
        }

        loop {
            match self.receiver.as_ref().unwrap().try_recv() {
                Ok(message) => {
                    match message {
                        Ok(result) => {
                            results.push(Ok(BackupSuccess::new(&result)));
                        }
                        Err(err) => {
                            if err.message == "Backup interrupted" {
                                for handle in &kill_handles {
                                    if let Err(err) = handle.lock().unwrap().kill() {
                                        self.logger.log(
                                            &format!("Error killing process: {:?}", err),
                                            LogLevel::Error,
                                        );
                                        results
                                            .push(Err(BackupError::new(err.to_string().as_str())));
                                    }
                                }
                                for join_handle in join_handles {
                                    if let Err(err) = join_handle.join() {
                                        self.logger.log(
                                            &format!("Error joining thread: {:?}", err),
                                            LogLevel::Error,
                                        );
                                    }
                                }
                                cleanup_temp_containers(&self.temp_containers, &self.logger);
                                self.logger
                                    .reset_cursor_after_timers(self.dest_paths.len() as u16);
                                self.logger.log(
                                    "Backup interrupted, press Ctrl+C again to force exit",
                                    LogLevel::Warning,
                                );

                                results.push(Err(BackupError::new("Backup interrupted")));
                                return results;
                            }
                            results.push(Err(err));
                        }
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
                    continue;
                }
            }
        }
    }
}

fn cleanup_temp_containers(temp_containers: &Arc<Mutex<HashSet<String>>>, logger: &Logger) {
    let containers: Vec<String> = match temp_containers.lock() {
        Ok(container_set) => container_set.iter().cloned().collect(),
        Err(_) => {
            logger.log("Failed to acquire temp container lock", LogLevel::Warning);
            return;
        }
    };

    for container in containers {
        if let Err(err) = std::process::Command::new("docker")
            .args(["rm", "-f", &container])
            .status()
        {
            logger.log(
                &format!("Failed to remove temp container {}: {}", container, err),
                LogLevel::Warning,
            );
        }
    }
}
