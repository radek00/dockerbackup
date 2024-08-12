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

        if !running_containers.is_empty() {
            handle_containers(&running_containers, "stop")?;
        }

        if let Err(errors) = self.run() {
            if !running_containers.is_empty() {
                handle_containers(&running_containers, "start")?;
            }
            for err in errors {
                err.notify(&self);
            }
        } else if !running_containers.is_empty() {
            handle_containers(&running_containers, "start")?;
        }
        Ok(())
    }
    fn run(&self) -> Result<(), Vec<BackupError>> {
        println!("Backup started...");
        let mut result_count: usize = 0;
        let mut errors: Vec<BackupError> = Vec::new();

        let mut backup_handles: Vec<(Arc<Mutex<Child>>, &str)> = Vec::new();

        for dest in &self.dest_path {
            if dest.0.contains('@') {
                let ssh_path_parts: Vec<&str> = dest.0.splitn(2, ':').collect();

                if ssh_path_parts.len() != 2 {
                    errors.push(BackupError::new("Invalid ssh path"));
                    result_count += 1;
                    continue;
                }
                match self.ssh_backup(ssh_path_parts, &dest.1) {
                    Ok(child) => {
                        backup_handles.push((Arc::new(Mutex::new(child)), "Ssh"));
                    }
                    Err(err) => {
                        errors.push(err);
                        result_count += 1;
                        continue;
                    }
                }
            } else {
                let dest_path = Path::new(&dest.0);
                if let Err(err) = create_new_dir(dest_path, &self.new_dir) {
                    errors.push(err);
                    result_count += 1;
                    continue;
                }
                match self.local_rsync_backup(dest_path) {
                    Ok(child) => {
                        backup_handles.push((Arc::new(Mutex::new(child)), "Rsync"));
                    }
                    Err(err) => {
                        errors.push(err);
                        result_count += 1;
                        continue;
                    }
                }
            };
        }

        if result_count == self.dest_path.len() {
            return Err(errors);
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
                                    if let Err(err) = handle.0.lock().unwrap().kill() {
                                        eprintln!("Error killing process: {:?}", err);
                                        errors.push(BackupError::new(err.to_string().as_str()));
                                    }
                                }
                                for join_handle in join_handles {
                                    if let Err(err) = join_handle.join() {
                                        eprintln!("Error joining thread: {:?}", err);
                                    }
                                }
                                errors.push(BackupError::new("Backup interrupted"));
                                return Err(errors);
                            }
                            println!("Err result: {}", err.message);
                            errors.push(err);
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

                        if !errors.is_empty() {
                            return Err(errors);
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
    fn local_rsync_backup(&self, dest_path: &Path) -> Result<Child, BackupError> {
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
    fn ssh_backup(
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
