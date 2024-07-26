use backup_error::BackupError;
use chrono::{self, Datelike};
use clap::builder::styling::{AnsiColor, Effects, Styles};
use notification::{send_notification, Discord, Gotify};
use std::path::{Path, PathBuf};
use std::process::{exit, Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::thread;
use utils::{
    check_docker, check_running_containers, create_new_dir, exclude_dirs, handle_containers,
    validate_destination_path,
};

mod backup_error;
mod notification;
mod utils;

#[derive(Clone, PartialEq, Eq)]
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
    dest_path: (String, TargetOs),
    new_dir: String,
    volume_path: PathBuf,
    excluded_directories: Vec<String>,
    gotify_url: Option<String>,
    discord_url: Option<String>,
    receiver: Option<Receiver<()>>,
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
                .help("Backup destination path. Accepts local or remote ssh path. Target os must be provided with ssh paths. [/backup or user@host:/backup, windows]")
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
                .remove_one::<(String, TargetOs)>("dest_path")
                .unwrap(),
            new_dir,
            volume_path: matches.remove_one::<PathBuf>("volume_path").unwrap(),
            excluded_directories,
            gotify_url: matches.remove_one::<String>("gotify_url"),
            discord_url: matches.remove_one::<String>("discord_url"),
            receiver: None,
        }
    }
    fn notify(&self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(gotify_url) = &self.gotify_url {
            send_notification::<Gotify>(Gotify {
                message: None,
                success: true,
                url: gotify_url,
            })?;
        }

        if let Some(dc_url) = &self.discord_url {
            send_notification::<Discord>(Discord {
                message: None,
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

        println!("Backup started...");
        if !running_containers.is_empty() {
            println!("Stopping containers...");
            handle_containers(&running_containers, "stop")?;
            self.run(Some(&running_containers))?;
            println!("Starting containers...");
            handle_containers(&running_containers, "start")?;
        } else {
            self.run(None)?;
        }

        Ok(())
    }
    pub fn backup_volumes(mut self) -> Self {
        let (sender, receiver): (mpsc::Sender<()>, mpsc::Receiver<()>) = mpsc::channel();
        let mut call_count = 0;
        ctrlc::set_handler(move || {
            if self.receiver.is_some() {
                if call_count == 0 {
                    println!("Backup interrputed, starting containers... Press Ctrl+C again to force exit");
                    sender
                        .send(())
                        .expect("Could not send signal through channel");
                    call_count += 1;
                } else {
                    println!("Forcing exit...");
                    exit(1);
                }
            } else {
                println!("Exiting...");
                exit(1);
            }
        })
        .expect("Error setting Ctrl-C handler");

        self.receiver = Some(receiver);

        self.backup().unwrap_or_else(|err| {
            err.notify(&self);
        });
        self.receiver = None;
        self.notify().unwrap_or_else(|err| {
            println!("Notification error: {}", err);
        });
        self
    }
    fn run(&self, running_containers: Option<&Vec<&str>>) -> Result<bool, BackupError> {
        let error_type;
        let mut backup_handle = if self.dest_path.0.contains('@') {
            error_type = "Ssh";
            self.ssh_backup()
        } else {
            error_type = "Rsync";
            self.local_rsync_backup()
        }?;

        loop {
            if let Ok(exist_status) = backup_handle.try_wait() {
                if let Some(status) = exist_status {
                    println!("Status {:?}", status);
                    if status.success() {
                        return Ok(true);
                    }
                    return Err(BackupError::new(&format!("{} backup failed", error_type)));
                } else if self.receiver.as_ref().unwrap().try_recv().is_ok() {
                    println!("Received message");
                    backup_handle.kill().expect("Failed to kill backup process");
                    if let Some(containers) = running_containers {
                        handle_containers(containers, "start")?;
                    }
                    return Err(BackupError::new("Backup interrupted"));
                }
            }
        }
    }
    fn local_rsync_backup(&self) -> Result<Child, BackupError> {
        let dest_path = Path::new(&self.dest_path.0);
        if !create_new_dir(dest_path, &self.new_dir)? {
            return Err(BackupError::new("Error creating new directory"));
        }

        let mut rsync = Command::new("rsync");

        exclude_dirs(&mut rsync, &self.excluded_directories);

        let exec_rsync = rsync
            .arg("-az")
            .arg(&self.volume_path)
            .arg(dest_path.join(&self.new_dir))
            .spawn()?;

        Ok(exec_rsync)
    }
    fn ssh_backup(&self) -> Result<Child, BackupError> {
        let mut tar_volumes = Command::new("tar");

        tar_volumes.arg("-cf-").arg("-C").arg(&self.volume_path);

        exclude_dirs(&mut tar_volumes, &self.excluded_directories);

        let tar_exec = tar_volumes.arg(".").stdout(Stdio::piped()).spawn()?;

        let ssh_path_parts: Vec<&str> = self.dest_path.0.splitn(2, ':').collect();

        if ssh_path_parts.len() != 2 {
            return Err(BackupError::new("Invalid ssh path"));
        }

        let dest_path = append_to_path(ssh_path_parts[1], &self.new_dir, &self.dest_path.1);

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
            .spawn()?;

        return Ok(ssh);
    }
}

fn append_to_path(path: &str, new_dir: &String, target_os: &TargetOs) -> String {
    if target_os == &TargetOs::Windows {
        format!("{}\\{}", path, new_dir)
    } else {
        format!("{}/{}", path, new_dir)
    }
}
