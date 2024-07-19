use chrono::{self, Datelike};
use notification::{send_notification, Discord, Gotify};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use utils::{
    check_docker, check_running_containers, create_new_dir, exclude_dirs, handle_containers,
};

mod notification;
mod utils;

pub struct DockerBackup {
    pub dest_path: String,
    pub new_dir: String,
    pub volume_path: PathBuf,
    pub excluded_directories: Vec<String>,
    pub gotify_url: Option<String>,
    pub discord_url: Option<String>,
}

impl DockerBackup {
    pub fn build() -> DockerBackup {
        let date = chrono::Local::now();
        let new_dir = format!("{}-{}-{}", date.year(), date.month(), date.day());

        let mut matches = clap::Command::new("Docker Backup")
            .version("0.1.0")
            .author("radek00")
            .about("Simple docker backup tool to perform backups to local destination or remote ssh server")
            .arg(clap::Arg::new("dest_path")
                .help("Backup destination path. Accepts local or remote ssh path. Example: /backup or user@host:/backup")
                .required(true)
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
            dest_path: matches.remove_one::<String>("dest_path").unwrap(),
            new_dir,
            volume_path: matches.remove_one::<PathBuf>("volume_path").unwrap(),
            excluded_directories,
            gotify_url: matches.remove_one::<String>("gotify_url"),
            discord_url: matches.remove_one::<String>("discord_url"),
        }
    }
    pub fn backup(&self) -> Result<(), Box<dyn std::error::Error>> {
        check_docker()?;
        let mut err_message = String::from("");
        let containers = check_running_containers()?;
        let mut running_containers: Vec<&str> = containers.trim().split('\n').collect();
        running_containers.retain(|&x| !x.is_empty());

        let err_closure = |err| {
            err_message = format!("{}", err);
            false
        };

        let backup_status;

        if !running_containers.is_empty() {
            println!("Stopping containers...");
            handle_containers(&running_containers, "stop")?;
            backup_status = self.run().unwrap_or_else(err_closure);
            println!("Starting containers...");
            handle_containers(&running_containers, "start")?;
        } else {
            backup_status = self.run().unwrap_or_else(err_closure);
        }

        if let Some(gotify_url) = &self.gotify_url {
            send_notification::<Gotify>(Gotify {
                err_message: &err_message,
                success: backup_status,
                url: gotify_url,
            })?;
        }

        if let Some(dc_url) = &self.discord_url {
            send_notification::<Discord>(Discord {
                err_message: &err_message,
                success: backup_status,
                url: dc_url,
            })?;
        }
        Ok(())
    }
    fn run(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let backup_status = if self.dest_path.contains('@') {
            self.ssh_backup()
        } else {
            self.local_rsync_backup()
        }?;

        Ok(backup_status)
    }
    fn local_rsync_backup(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let dest_path = Path::new(&self.dest_path);
        if !create_new_dir(dest_path, &self.new_dir)? {
            return Err(Box::from("Could not create directory"));
        }

        let mut rsync = Command::new("rsync");

        exclude_dirs(&mut rsync, &self.excluded_directories);

        let exec_rsync = rsync
            .arg("-az")
            .arg(&self.volume_path)
            .arg(dest_path.join(&self.new_dir))
            .status()?;
        if exec_rsync.success() {
            return Ok(true);
        }
        Err(Box::from("Rsync backup failed"))
    }
    fn ssh_backup(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let mut tar_volumes = Command::new("tar");

        tar_volumes.arg("-cf-").arg("-C").arg(&self.volume_path);

        exclude_dirs(&mut tar_volumes, &self.excluded_directories);

        let tar_exec = tar_volumes.arg(".").stdout(Stdio::piped()).spawn()?;

        let ssh_path_parts: Vec<&str> = self
            .dest_path
            .splitn(2, std::path::MAIN_SEPARATOR)
            .collect();

        if ssh_path_parts.len() != 2 {
            return Err(Box::from("Wrong path format"));
        }

        let dest_path = Path::new(ssh_path_parts[1]).join(&self.new_dir);

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
            .output()?;

        if ssh.status.success() {
            return Ok(true);
        }
        Err(Box::from(format!(
            "Ssh backup failed: {}",
            String::from_utf8_lossy(&ssh.stderr)
        )))
    }
}
