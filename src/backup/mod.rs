use backup_result::{BackupError, BackupSuccess};
use chrono::{self, Datelike};
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::ArgAction;
use std::collections::HashSet;
use std::io::{stdout, BufReader, Read, Stdout};
use std::path::{Path, PathBuf};
use std::process::{exit, Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;
use utils::{
    check_available_space, check_docker, check_running_containers, clear_terminal, create_new_dir,
    exclude_volumes, get_elapsed_time, get_volumes_size, handle_containers, hide_cursor,
    parse_destination_path, print_elapsed_time, reset_cursor_after_timers, show_cursor,
    BackupDestination,
};

mod backup_result;
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
    dest_paths: Vec<BackupDestination>,
    new_dir: String,
    volume_path: PathBuf,
    excluded_containers: Vec<String>,
    excluded_volumes: Vec<String>,
    gotify_url: Option<String>,
    discord_url: Option<String>,
    receiver: Option<Receiver<Result<String, BackupError>>>,
    sender: Option<Sender<Result<String, BackupError>>>,
    stdout: Arc<Mutex<Stdout>>,
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
                .remove_many::<BackupDestination>("dest_path")
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
            stdout: Arc::new(Mutex::new(stdout())),
        }
    }
    pub fn backup(mut self) -> Result<(), BackupError> {
        clear_terminal(&self.stdout);
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
        ctrlc::set_handler(move || {
            if call_count == 0 {
                sender_clone
                    .send(Err(BackupError::new("Backup interrupted")))
                    .unwrap();

                call_count += 1;
            } else {
                println!("Forcing exit...");
                exit(1);
            }
        })
        .expect("Error setting Ctrl-C handler");

        self.receiver = Some(receiver);
        self.sender = Some(sender);

        if !running_containers.is_empty() {
            println!("Stopping containers...");
            handle_containers(&running_containers, "stop")?;
        }

        hide_cursor(&self.stdout);
        let results = self.run();
        show_cursor(&self.stdout);

        if !running_containers.is_empty() {
            println!("Starting containers...");
            handle_containers(&running_containers, "start")?;
        }

        for result in results {
            match result {
                Ok(success) => {
                    success.notify(&self);
                }
                Err(err) => {
                    eprintln!("Error: {}", err);
                    err.notify(&self);
                }
            }
        }
        Ok(())
    }
    fn run(&self) -> Vec<Result<BackupSuccess, BackupError>> {
        println!("Backup started...");
        let mut results: Vec<Result<BackupSuccess, BackupError>> = Vec::new();

        let total_size = match get_volumes_size(&self.volume_path, &self.excluded_volumes) {
            Ok(size) => size,
            Err(err) => {
                results.push(Err(err));
                return results;
            }
        };

        println!("Total size to backup: {:.2} MB", total_size as f64 / (1024.0 * 1024.0));

        let mut backup_handles: Vec<(Arc<Mutex<Child>>, String)> = Vec::new();

        for dest in &self.dest_paths {
            if let Err(err) = check_available_space(dest, total_size) {
                results.push(Err(err));
                continue;
            }

            match dest {
                BackupDestination::Ssh {
                    host,
                    path,
                    target_os,
                } => match self.spawn_ssh_backup(host, path, target_os) {
                    Ok(child) => {
                        backup_handles.push((
                            Arc::new(Mutex::new(child)),
                            format!("SSH backup to destination {}:{}", host, path),
                        ));
                    }
                    Err(err) => {
                        results.push(Err(err));
                    }
                },
                BackupDestination::Local { path } => {
                    let dest_path = Path::new(path);
                    if let Err(err) = create_new_dir(dest_path, &self.new_dir) {
                        results.push(Err(err));
                        continue;
                    }
                    match self.spawn_local_rsync_backup(dest_path) {
                        Ok(child) => {
                            backup_handles.push((
                                Arc::new(Mutex::new(child)),
                                format!("Rsync backup to destination {}", path),
                            ));
                        }
                        Err(err) => {
                            results.push(Err(err));
                        }
                    }
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
            let stdout_mutex_clone = self.stdout.clone();
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
                                print_elapsed_time(idx, &msg, &stdout_mutex_clone);
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
                                        eprintln!("Failed to read stderr: {}", e);
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
                            print_elapsed_time(
                                idx,
                                &get_elapsed_time(
                                    timer,
                                    format!("\r{} running time", handle.1).as_str(),
                                ),
                                &stdout_mutex_clone,
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
                                        eprintln!("Error killing process: {:?}", err);
                                        results
                                            .push(Err(BackupError::new(err.to_string().as_str())));
                                    }
                                }
                                for join_handle in join_handles {
                                    if let Err(err) = join_handle.join() {
                                        eprintln!("Error joining thread: {:?}", err);
                                    }
                                }
                                reset_cursor_after_timers(
                                    self.dest_paths.len() as u16,
                                    &self.stdout,
                                );
                                println!("Backup interrputed, press Ctrl+C again to force exit");

                                results.push(Err(BackupError::new("Backup interrupted")));
                                return results;
                            }
                            results.push(Err(err));
                        }
                    }
                    if results.len() == self.dest_paths.len() {
                        reset_cursor_after_timers(self.dest_paths.len() as u16, &self.stdout);
                        println!("All backups finished");
                        for join_handle in join_handles {
                            if let Err(err) = join_handle.join() {
                                eprintln!("Error joining thread: {:?}", err);
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
    fn spawn_local_rsync_backup(&self, dest_path: &Path) -> Result<Child, BackupError> {
        let mut rsync = Command::new("rsync");

        exclude_volumes(&mut rsync, &self.excluded_volumes, &self.volume_path)?;

        let exec_rsync = rsync
            .arg("-aW")
            .arg(&self.volume_path)
            .arg(dest_path.join(&self.new_dir))
            .stderr(Stdio::piped())
            .spawn()?;

        Ok(exec_rsync)
    }
    fn spawn_ssh_backup(
        &self,
        host: &str,
        path: &str,
        target_os: &TargetOs,
    ) -> Result<Child, BackupError> {
        let mut tar_volumes = Command::new("tar");

        tar_volumes.arg("-cf-").arg("-C").arg(&self.volume_path);

        exclude_volumes(&mut tar_volumes, &self.excluded_volumes, &self.volume_path)?;

        let tar_exec = tar_volumes.arg(".").stdout(Stdio::piped()).spawn()?;

        let dest_path = append_to_path(path, &self.new_dir, target_os);

        let ssh = Command::new("ssh")
            .arg(host)
            .arg("mkdir")
            .arg(&dest_path)
            .arg("&&")
            .arg("tar")
            .arg("-C")
            .arg(dest_path)
            .arg("-xf-")
            .stdin(Stdio::from(tar_exec.stdout.unwrap()))
            .stderr(Stdio::piped())
            .spawn()?;

        Ok(ssh)
    }
}

fn append_to_path(path: &str, new_dir: &String, target_os: &TargetOs) -> String {
    if target_os == &TargetOs::Windows {
        format!("{}\\{}", path, new_dir)
    } else {
        format!("{}/{}", path, new_dir)
    }
}
