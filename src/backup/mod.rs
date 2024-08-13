use backup_error::{BackupError, BackupSuccess};
use chrono::{self, Datelike};
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::ArgAction;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{exit, Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use utils::{
    check_docker, check_running_containers, create_new_dir, exclude_dirs, handle_containers,
    validate_destination_path,
};

mod backup_error;
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
    dest_paths: Vec<(String, TargetOs)>,
    new_dir: String,
    volume_path: PathBuf,
    excluded_directories: Vec<String>,
    gotify_url: Option<String>,
    discord_url: Option<String>,
    receiver: Option<Receiver<Result<String, BackupError>>>,
    sender: Option<Sender<Result<String, BackupError>>>,
}

impl DockerBackup {
    pub fn build() -> DockerBackup {
        let date = chrono::Local::now();
        let new_dir = format!("{}-{}-{}", date.year(), date.month(), date.day());

        let mut matches = clap::Command::new("Docker Backup")
            .version(env!("CARGO_PKG_VERSION"))
            .author("radek00")
            .about("Simple docker backup tool to perform backups to local destination or remote ssh server")
            .styles(Styles::styled()
            .header(AnsiColor::BrightGreen.on_default() | Effects::BOLD)
            .usage(AnsiColor::Yellow.on_default() | Effects::BOLD)
            .placeholder(AnsiColor::Yellow.on_default()))
            .arg(clap::Arg::new("dest_path")
                .help("Backup destination path. This argument can be used multiple times and each path must be in the following format: [/backup or user@host:/backup, windows]. Target os must be specified with ssh paths.")
                .required(true)
                .num_args(1..)
                .action(ArgAction::Append)
                .value_parser(validate_destination_path)
                .short('d')
            .long("destination"))
            .arg(clap::Arg::new("volume_path")
                .help("Path to docker volumes directory")
                .value_parser(clap::value_parser!(PathBuf))
                .default_value("/var/lib/docker/volumes")
                .required(false)
                .long("volumes"))
            .arg(clap::Arg::new("excluded_volumes")
                .help("Volumes to exclude from the backup")
                .required(false)
                .short('e')
                .long("exclude")
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

        let mut excluded_directories = match matches.remove_many::<String>("excluded_volumes") {
            Some(dirs) => dirs.collect(),
            None => vec![],
        };

        excluded_directories.push(String::from("backingFsBlockDev"));

        DockerBackup {
            dest_paths: matches
                .remove_many::<(String, TargetOs)>("dest_path")
                .unwrap()
                .collect(),
            new_dir,
            volume_path: matches.remove_one::<PathBuf>("volume_path").unwrap(),
            excluded_directories,
            gotify_url: matches.remove_one::<String>("gotify_url"),
            discord_url: matches.remove_one::<String>("discord_url"),
            receiver: None,
            sender: None,
        }
    }
    pub fn backup(mut self) -> Result<(), BackupError> {
        check_docker()?;
        let containers = check_running_containers()?;
        let mut running_containers: Vec<&str> = containers.trim().split('\n').collect();
        running_containers.retain(|&x| !x.is_empty());

        let (sender, receiver): BackupChannel = mpsc::channel();
        let mut call_count = 0;

        let sender_clone = sender.clone();
        ctrlc::set_handler(move || {
            if call_count == 0 {
                println!();
                println!("Backup interrputed, press Ctrl+C again to force exit");
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

        let results = self.run();

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
                    err.notify(&self);
                }
            }
        }
        Ok(())
    }
    fn run(&self) -> Vec<Result<BackupSuccess, BackupError>> {
        println!("Backup started...");
        let mut results: Vec<Result<BackupSuccess, BackupError>> = Vec::new();

        let mut backup_handles: Vec<(Arc<Mutex<Child>>, &str)> = Vec::new();

        for dest in &self.dest_paths {
            if dest.0.contains('@') {
                let ssh_path_parts: Vec<&str> = dest.0.splitn(2, ':').collect();

                if ssh_path_parts.len() != 2 {
                    results.push(Err(BackupError::new("Invalid ssh path")));
                    continue;
                }

                match self.spawn_ssh_backup(ssh_path_parts, &dest.1) {
                    Ok(child) => {
                        backup_handles.push((Arc::new(Mutex::new(child)), "Ssh"));
                    }
                    Err(err) => {
                        results.push(Err(err));
                    }
                }
            } else {
                let dest_path = Path::new(&dest.0);
                if let Err(err) = create_new_dir(dest_path, &self.new_dir) {
                    results.push(Err(err));
                    continue;
                }
                match self.spawn_local_rsync_backup(dest_path) {
                    Ok(child) => {
                        backup_handles.push((Arc::new(Mutex::new(child)), "Rsync"));
                    }
                    Err(err) => {
                        results.push(Err(err));
                    }
                }
            };
        }

        if results.len() == self.dest_paths.len() {
            return results;
        }

        let sender = self.sender.as_ref().unwrap();
        let mut join_handles: Vec<thread::JoinHandle<()>> = Vec::new();

        for handle in &backup_handles {
            let sender_clone = sender.clone();
            let handle = handle.clone();
            let join_handle = thread::spawn(move || {
                let stderr = handle.0.lock().unwrap().stderr.take();
                let mut stderr_reader = stderr.map(BufReader::new);
                let mut buffer = Vec::new();

                if let Ok(status) = handle.0.lock().unwrap().wait() {
                    if status.success() {
                        sender_clone
                            .send(Ok(format!("{} backup successful", handle.1)))
                            .unwrap();
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
                            .send(Err(BackupError::new(&format!("{} backup error", handle.1))))
                            .unwrap();
                        return;
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
                                results.push(Err(BackupError::new("Backup interrupted")));
                                return results;
                            }
                            results.push(Err(err));
                        }
                    }
                    if results.len() == self.dest_paths.len() {
                        println!("Backup finished");
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

        exclude_dirs(&mut rsync, &self.excluded_directories);

        let exec_rsync = rsync
            .arg("-az")
            .arg(&self.volume_path)
            .arg(dest_path.join(&self.new_dir))
            .stderr(Stdio::piped())
            .spawn()?;

        Ok(exec_rsync)
    }
    fn spawn_ssh_backup(
        &self,
        ssh_path_parts: Vec<&str>,
        target_os: &TargetOs,
    ) -> Result<Child, BackupError> {
        let mut tar_volumes = Command::new("tar");

        tar_volumes.arg("-cf-").arg("-C").arg(&self.volume_path);

        exclude_dirs(&mut tar_volumes, &self.excluded_directories);

        let tar_exec = tar_volumes.arg(".").stdout(Stdio::piped()).spawn()?;

        let dest_path = append_to_path(ssh_path_parts[1], &self.new_dir, target_os);

        let ssh = Command::new("ssh")
            .arg(ssh_path_parts[0])
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
