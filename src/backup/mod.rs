use backup_error::BackupError;
use chrono::{self, Datelike};
use clap::builder::styling::{AnsiColor, Effects, Styles};
use notification::{send_notification, Discord, Gotify};
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
    pub dest_path: Vec<(String, TargetOs)>,
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
                .help("Accepts multile local or remote ssh destination paths. Destination paths must be separated by ^ and in format [/backup or user@host:/backup, windows]. Target os must be specified with ssh paths.")
                .required(true)
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
            dest_path: matches
                .remove_one::<Vec<(String, TargetOs)>>("dest_path")
                .unwrap(),
            new_dir,
            volume_path: matches.remove_one::<PathBuf>("volume_path").unwrap(),
            excluded_directories,
            gotify_url: matches.remove_one::<String>("gotify_url"),
            discord_url: matches.remove_one::<String>("discord_url"),
            receiver: None,
            sender: None,
        }
    }
    fn notify(&self, message: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(gotify_url) = &self.gotify_url {
            send_notification::<Gotify>(Gotify {
                message: message.clone(),
                success: true,
                url: gotify_url,
            })?;
        }

        if let Some(dc_url) = &self.discord_url {
            send_notification::<Discord>(Discord {
                message,
                success: true,
                url: dc_url,
            })?;
        }

        Ok(())
    }
    fn backup(&self) -> Result<(), BackupError> {
        check_docker()?;
        let containers = check_running_containers()?;
        let mut running_containers: Vec<&str> = containers.trim().split('\n').collect();
        running_containers.retain(|&x| !x.is_empty());

        if !running_containers.is_empty() {
            println!("Stopping containers...");
            handle_containers(&running_containers, "stop")?;
            if let Err(err) = self.run() {
                println!("Backup error: {}", err.message);
                println!("Starting containers...");
                handle_containers(&running_containers, "start")?;
                return Err(err);
            }
            println!("Starting containers...");
            handle_containers(&running_containers, "start")?;
        } else {
            self.run()?;
        }
        Ok(())
    }
    pub fn backup_volumes(mut self) -> Self {
        let (sender, receiver): BackupChannel = mpsc::channel();
        let mut call_count = 0;

        let sender_clone = sender.clone();
        ctrlc::set_handler(move || {
            if call_count == 0 {
                println!();
                println!("Backup interrputed, press Ctrl+C again to force exit");
                sender_clone
                    .send(Err(BackupError::new("Backup interrupted")))
                    .expect("Could not send signal through channel");
                call_count += 1;
            } else {
                println!("Forcing exit...");
                exit(1);
            }
        })
        .expect("Error setting Ctrl-C handler");

        self.receiver = Some(receiver);
        self.sender = Some(sender);

        if let Err(err) = self.backup() {
            err.notify(&self);
            return self;
        }
        self
    }
    fn run(&self) -> Result<(), BackupError> {
        println!("Backup started...");
        //let error_type;

        let mut backup_handles: Vec<(Arc<Mutex<Child>>, &str)> = Vec::new();

        for dest in &self.dest_path {
            if dest.0.contains('@') {
                //error_type = "Ssh";
                backup_handles.push((Arc::new(Mutex::new(self.ssh_backup(dest)?)), "Ssh"));
            } else {
                //error_type = "Rsync";
                backup_handles.push((
                    Arc::new(Mutex::new(self.local_rsync_backup(dest)?)),
                    "Rsync",
                ));
            };
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
                    println!("lock acquired");
                    if status.success() {
                        //send
                        thread::sleep(std::time::Duration::from_secs(10));
                        sender_clone
                            .send(Ok(format!("{} backup successful", handle.1)))
                            .expect("Could not send signal through channel");
                    } else if let Some(reader) = stderr_reader.as_mut() {
                        match reader.read_to_end(&mut buffer) {
                            Ok(_) => {
                                let stderr_output = String::from_utf8_lossy(&buffer);
                                sender_clone
                                    .send(Err(BackupError::new(&stderr_output)))
                                    .expect("Could not send signal through channel");
                                return;
                            }
                            Err(e) => {
                                eprintln!("Failed to read stderr: {}", e);
                                sender_clone
                                    .send(Err(BackupError::new(&format!(
                                        "{} backup error",
                                        handle.1
                                    ))))
                                    .expect("Could not send signal through channel");
                                return;
                            }
                        }
                    } else {
                        sender_clone
                            .send(Err(BackupError::new(&format!("{} backup error", handle.1))))
                            .expect("Could not send signal through channel");
                        return;
                    }
                }
            });
            join_handles.push(join_handle);
        }

        let mut result_count: usize = 0;
        loop {
            match self.receiver.as_ref().unwrap().try_recv() {
                Ok(message) => {
                    match message {
                        Ok(result) => {
                            println!("Ok result");
                            self.notify(Some(result)).unwrap_or_else(|err| {
                                eprintln!("Error sending notification: {}", err);
                            });
                            result_count += 1;
                        }
                        Err(err) => {
                            if err.message == "Backup interrupted" {
                                for handle in backup_handles {
                                    handle.0.lock().unwrap().kill()?;
                                }
                                for join_handle in join_handles {
                                    if let Err(err) = join_handle.join() {
                                        eprintln!("Error joining thread: {:?}", err);
                                    }
                                }
                                return Err(err);
                            }
                            println!("Err result: {}", err.message);
                            err.notify(self);
                            result_count += 1;
                        }
                    }
                    if result_count == self.dest_path.len() {
                        println!("Backup finished");
                        for join_handle in join_handles {
                            if let Err(err) = join_handle.join() {
                                eprintln!("Error joining thread: {:?}", err);
                            }
                        }
                        return Ok(());
                    }
                }
                Err(_) => {
                    continue;
                }
            }
        }
    }
    fn local_rsync_backup(&self, dest_path: &(String, TargetOs)) -> Result<Child, BackupError> {
        let dest_path = Path::new(&dest_path.0);
        if !create_new_dir(dest_path, &self.new_dir)? {
            return Err(BackupError::new("Error creating new directory"));
        }

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
    fn ssh_backup(&self, dest_path: &(String, TargetOs)) -> Result<Child, BackupError> {
        let mut tar_volumes = Command::new("tar");

        tar_volumes.arg("-cf-").arg("-C").arg(&self.volume_path);

        exclude_dirs(&mut tar_volumes, &self.excluded_directories);

        let tar_exec = tar_volumes.arg(".").stdout(Stdio::piped()).spawn()?;

        let ssh_path_parts: Vec<&str> = dest_path.0.splitn(2, ':').collect();

        if ssh_path_parts.len() != 2 {
            return Err(BackupError::new("Invalid ssh path"));
        }

        let dest_path = append_to_path(ssh_path_parts[1], &self.new_dir, &dest_path.1);

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
