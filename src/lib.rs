use chrono::{self, Datelike};
use notification::send_notification;
use notification::Discord;
use notification::Gotify;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

mod notification;

pub struct Config {
    pub dest_path: String,
    pub new_dir: String,
    pub volume_path: PathBuf,
    pub excluded_directories: Vec<String>,
    pub gotify_url: Option<String>,
    pub discord_url: Option<String>,
}

impl Config {
    pub fn build() -> Result<Config, &'static str> {
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

        Ok(Config {
            dest_path: matches.remove_one::<String>("dest_path").unwrap(),
            new_dir,
            volume_path: matches.remove_one::<PathBuf>("volume_path").unwrap(),
            excluded_directories,
            gotify_url: matches.remove_one::<String>("gotify_url"),
            discord_url: matches.remove_one::<String>("discord_url"),
        })
    }
}

pub fn backup() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::build()?;
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
        backup_status = run(&config).unwrap_or_else(err_closure);
        println!("Starting containers...");
        handle_containers(&running_containers, "start")?;
    } else {
        backup_status = run(&config).unwrap_or_else(err_closure);
    }

    if let Some(gotify_url) = config.gotify_url {
        send_notification::<Gotify>(Gotify {
            err_message: &err_message,
            success: backup_status,
            url: &gotify_url,
        })?;
    }

    if let Some(dc_url) = config.discord_url {
        send_notification::<Discord>(Discord {
            err_message: &err_message,
            success: backup_status,
            url: &dc_url,
        })?;
    }
    Ok(())
}

fn run(config: &Config) -> Result<bool, Box<dyn std::error::Error>> {
    let backup_status = if config.dest_path.contains('@') {
        ssh_backup(config)
    } else {
        local_rsync_backup(config)
    }?;

    Ok(backup_status)
}

fn check_docker() -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("docker").arg("--version").status()?;
    if status.success() {
        return Ok(());
    }
    Err(Box::from("Can't continue without Docker installed"))
}

fn check_running_containers() -> Result<String, Box<dyn std::error::Error>> {
    let running_containers = Command::new("docker")
        .args(["container", "ls", "-q"])
        .output()?;
    let containers_list = String::from_utf8(running_containers.stdout)?;
    Ok(containers_list)
}

fn local_rsync_backup(config: &Config) -> Result<bool, Box<dyn std::error::Error>> {
    let dest_path = Path::new(&config.dest_path);
    if !create_new_dir(dest_path, &config.new_dir)? {
        return Err(Box::from("Could not create directory"));
    }

    let mut rsync = Command::new("rsync");

    exclude_dirs(&mut rsync, &config.excluded_directories);

    let exec_rsync = rsync
        .arg("-az")
        .arg(&config.volume_path)
        .arg(dest_path.join(&config.new_dir))
        .status()?;
    if exec_rsync.success() {
        return Ok(true);
    }
    Err(Box::from("Rsync backup failed"))
}

fn ssh_backup(config: &Config) -> Result<bool, Box<dyn std::error::Error>> {
    let mut tar_volumes = Command::new("tar");

    tar_volumes.arg("-cf-").arg("-C").arg(&config.volume_path);

    exclude_dirs(&mut tar_volumes, &config.excluded_directories);

    let tar_exec = tar_volumes.arg(".").stdout(Stdio::piped()).spawn()?;

    let ssh_path_parts: Vec<&str> = config
        .dest_path
        .splitn(2, std::path::MAIN_SEPARATOR)
        .collect();

    if ssh_path_parts.len() != 2 {
        return Err(Box::from("Wrong path format"));
    }

    let dest_path = Path::new(ssh_path_parts[1]).join(&config.new_dir);

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

fn exclude_dirs(command: &mut Command, dirs_to_exclude: &Vec<String>) {
    for dir in dirs_to_exclude {
        command.arg(format!("--exclude={}", dir));
    }
}

fn create_new_dir(dest_path: &Path, new_dir: &String) -> Result<bool, Box<dyn std::error::Error>> {
    let new_dir = Command::new("mkdir")
        .arg("-p")
        .arg(dest_path.join(new_dir))
        .status()?;
    Ok(new_dir.success())
}

fn handle_containers(
    containers: &Vec<&str>,
    command: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let cmd_result = Command::new("docker")
        .arg(command)
        .args(containers)
        .status()?;
    if cmd_result.success() {
        return Ok(());
    }
    Err(Box::from("Failed to handle containers"))
}
