use backup_result::{BackupError, BackupSuccess};
use chrono::{self, Datelike};
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::ArgAction;
use crossterm::style::Color;
use std::collections::HashSet;
use std::io::{stdout, BufReader, Read};
use std::path::PathBuf;
use std::process::{exit, Child};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;
use utils::{
    check_docker, check_running_containers, get_elapsed_time, get_volumes_size, handle_containers,
    parse_destination_path,
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
    volume_path: PathBuf,
    excluded_containers: Vec<String>,
    excluded_volumes: Vec<String>,
    gotify_url: Option<String>,
    discord_url: Option<String>,
    receiver: Option<Receiver<Result<String, BackupError>>>,
    sender: Option<Sender<Result<String, BackupError>>>,
    logger: Arc<Logger>,
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
            .arg(clap::Arg::new("volume_path")
                .help("Path to docker volumes directory")
                .value_parser(clap::value_parser!(PathBuf))
                .default_value("/var/lib/docker/volumes")
                .required(false)
                .long("volumes"))
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

        excluded_volumes.push("backingFsBlockDev".to_string());

        DockerBackup {
            dest_paths: matches
                .remove_many::<Arc<dyn BackupDestination>>("dest_path")
                .unwrap()
                .collect(),
            new_dir,
            volume_path: matches.remove_one::<PathBuf>("volume_path").unwrap(),
            excluded_containers,
            excluded_volumes,
            gotify_url: matches.remove_one::<String>("gotify_url"),
            discord_url: matches.remove_one::<String>("discord_url"),
            receiver: None,
            sender: None,
            logger: Arc::new(Logger::new(stdout())),
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
        ctrlc::set_handler(move || {
            if call_count == 0 {
                sender_clone
                    .send(Err(BackupError::new("Backup interrupted")))
                    .unwrap();

                call_count += 1;
            } else {
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

        let total_size = match get_volumes_size(&self.volume_path, &self.excluded_volumes) {
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

        let mut backup_handles: Vec<(Arc<Mutex<Child>>, String)> = Vec::new();

        for dest in &self.dest_paths {
            if let Err(err) = dest.check_available_space(total_size) {
                results.push(Err(err));
                continue;
            }

            if let Err(err) = dest.prepare(&self.new_dir) {
                results.push(Err(err));
                continue;
            }

            match dest.spawn_backup(&self.volume_path, &self.excluded_volumes, &self.new_dir) {
                Ok(child) => {
                    backup_handles.push((
                        Arc::new(Mutex::new(child)),
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
            let join_handle = thread::spawn(move || {
                let timer = Instant::now();
                let stderr = handle.0.lock().unwrap().stderr.take();
                let mut stderr_reader = stderr.map(BufReader::new);
                let mut buffer = Vec::new();
                loop {
                    if let Ok(status) = handle.0.lock().unwrap().try_wait() {
                        if let Some(status) = status {
                            if status.success() {
                                let msg = get_elapsed_time(
                                    timer,
                                    format!("{} completed successfully in", handle.1).as_str(),
                                );
                                logger_clone.log_elapsed_time(idx, &msg, Color::Green);
                                sender_clone.send(Ok(msg)).unwrap();
                                return;
                            } else if let Some(reader) = stderr_reader.as_mut() {
                                match reader.read_to_end(&mut buffer) {
                                    Ok(_) => {
                                        let stderr_output = String::from_utf8_lossy(&buffer);
                                        sender_clone
                                            .send(Err(BackupError::new(&stderr_output)))
                                            .unwrap();
                                        return;
                                    }
                                    Err(e) => {
                                        logger_clone.log(
                                            &format!("Failed to read stderr: {}", e),
                                            LogLevel::Error,
                                        );
                                        sender_clone
                                            .send(Err(BackupError::new(&format!(
                                                "{} backup error",
                                                handle.1
                                            ))))
                                            .unwrap();
                                        return;
                                    }
                                }
                            } else {
                                sender_clone
                                    .send(Err(BackupError::new(&format!(
                                        "{} backup error",
                                        handle.1
                                    ))))
                                    .unwrap();
                                return;
                            }
                        } else {
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
                        Ok(result) => {
                            results.push(Ok(BackupSuccess::new(&result)));
                        }
                        Err(err) => {
                            if err.message == "Backup interrupted" {
                                for handle in backup_handles {
                                    if let Err(err) = handle.0.lock().unwrap().kill() {
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
                                self.logger
                                    .reset_cursor_after_timers(self.dest_paths.len() as u16);
                                self.logger.log(
                                    "Backup interrputed, press Ctrl+C again to force exit",
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
