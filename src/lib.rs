use std::process:: Stdio;
use std::process::Command;
use chrono::{self, Datelike};
use clap;
use notification::send_notification;

mod notification;

pub struct Config {
    pub dest_path: String,
    pub new_dir: String,
    pub volume_path: String,
    pub excluded_directories: Vec<String>,
}

impl Config {
    pub fn build() -> Result<Config, &'static str> {
        let date = chrono::Local::now();
        let new_dir = format!("{}-{}-{}", date.year(), date.month(), date.day());

        let matches = clap::Command::new("Docker Backup")
            .version("1.0")
            .author("Your Name <your.email@example.com>")
            .about("Performs a backup over SSH")
            .arg(clap::Arg::new("dest_path")
                .help("Backup destination path. Pass remote ssh path or local")
                .required(true)
                .short('d')
                .long("destiation"))
            .arg(clap::Arg::new("volume_path")
                .help("Path to docker volumes")
                .default_value("/var/lib/docker/volumes")
                .required(false)
                .short('v')
                .long("volume_path"))
            .arg(clap::Arg::new("excluded_directories")
                .help("Directories to exclude from the backup")
                .required(false)
                .short('e')
                .long("exclude")
                .num_args(1..))
            .get_matches();

        let dest_path = matches.get_one::<String>("dest_path").unwrap().to_string();
        let volume_path = matches.get_one::<String>("volume_path").unwrap().to_string();
        let excluded_directories: Vec<String> = matches.get_many::<String>("excluded_directories").unwrap().map(| dir | String::from(dir)).collect();

        Ok(Config { dest_path, new_dir, volume_path, excluded_directories })
    } 
}

pub fn backup() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::build().unwrap();
    check_docker()?;
    let mut err_message = String::from("");
    let containers = check_running_containers()?;
    let mut running_containers: Vec<&str> = containers.trim().split("\n").collect();
    running_containers.retain(|&x| !x.is_empty());

    let err_closure = | err | {
        err_message = format!("{}", err);
        false
    };

    let backup_status;

    if running_containers.len() > 0 {
        println!("Stopping containers..."); 
        handle_containers(&running_containers, "stop")?;
        backup_status =  run(&config).unwrap_or_else(err_closure);
        println!("Starting containers...");
        handle_containers(&running_containers, "start")?;
    } else {
        backup_status = run(&config).unwrap_or_else(err_closure);
    }

    send_notification(backup_status, format!("{}", err_message))?;
    Ok(())
}

fn run(config: &Config) -> Result<bool, Box<dyn std::error::Error>> {

    let backup_status = if config.dest_path.contains("@") {
        ssh_backup(&config)
    } else {
        local_rsync_backup(&config)
    }?;

    Ok(backup_status)
}


fn check_docker() -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("docker").arg("--version").status()?;
    if status.success() { return Ok(()) }
    Err(Box::from("Can't continue without Docker installed"))
}

fn check_running_containers() -> Result<String, Box<dyn std::error::Error>> {
    let running_containers = Command::new("docker").args(["container", "ls", "-q"]).output()?;
    let containers_list = String::from_utf8(running_containers.stdout)?;
    Ok(containers_list)
}

fn local_rsync_backup(config: &Config) -> Result<bool, Box<dyn std::error::Error>> {
    if !create_new_dir(&config)? {
        return Err(Box::from("Could not create directory"))
    } 

    let mut rsync = Command::new("rsync");

    exclude_dirs(&mut rsync, &config.excluded_directories);

    let exec_rsync = rsync.arg("-az").arg(&config.volume_path).arg(format!("{}/{}", config.dest_path, config.new_dir)).status()?;
    if exec_rsync.success() { return  Ok(true) }
    Err(Box::from("Rsync backup failed"))
}

fn ssh_backup(config: &Config) -> Result<bool, Box<dyn std::error::Error>> {
    let mut tar_volumes = Command::new("tar");

    tar_volumes.arg("-cf-").arg("-C").arg(&config.volume_path);

    exclude_dirs(&mut tar_volumes, &config.excluded_directories);

    let tar_exec = tar_volumes.arg(".").stdout(Stdio::piped()).spawn()?;

    let path: Vec<&str> = config.dest_path.split("/").collect();

    let dest_path = append_to_path(path[1], &config.new_dir);

    let ssh = Command::new("ssh").arg(path[0])
    .arg("mkdir").arg(format!("{}\\{}", path[1], config.new_dir)).arg("&&")
    .arg("tar").arg("-C").arg(dest_path).arg("-xf-")
    .stdin(Stdio::from(tar_exec.stdout.unwrap())).status()?;

    if ssh.success() { return Ok(true) } 
    Err(Box::from("Ssh backup failed"))
}

fn exclude_dirs(command: &mut Command, dirs_to_exclude: &Vec<String>) -> () {
    for dir in dirs_to_exclude {
        command.arg(format!("--exclude={}", dir));
    }
}

fn create_new_dir(config: &Config) -> Result<bool, Box<dyn std::error::Error>> {
    let new_dir = Command::new("mkdir").arg("-p").arg(format!("{}/{}", config.dest_path, config.new_dir)).status()?;
    Ok(new_dir.success())
}

fn handle_containers(containers: &Vec<&str>, command: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cmd_result = Command::new("docker").arg(command).args(containers).status()?;
    if cmd_result.success() { return Ok(()) }
    Err(Box::from("Failed to handle containers"))
}

fn append_to_path(path: &str, new_dir: &String) -> String {
    if path.contains("\\") {
        format!("{}\\{}", path, new_dir)
    } else {
        format!("{}/{}", path, new_dir)
    }
}