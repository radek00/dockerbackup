use std::process::{ Stdio };
use std::process:: { Command };
use std::env::{self};
use chrono::{self, Datelike};
use notification::send_notification;

mod notification;

pub struct Config {
    pub dest_path: String,
    pub new_dir: String,
    pub volume_path: String,
    pub excluded_directories: String,
}

impl Config {
    pub fn build(mut args: env::Args) -> Result<Config, &'static str> {
        args.next();

        let date = chrono::Local::now();

        let dest_path = match args.next() {
            Some(arg) => arg,
            None => return Err("No destination path provided")
            
        };

        let new_dir = format!("{}-{}-{}", date.year(), date.month(), date.day());
        let volume_path = match args.next() {
            Some(arg) => arg,
            None => return Err("No volume path provided")
        };

        let excluded_directories = match args.next() {
            Some(arg) => arg,
            None => String::new()
        };
        
        Ok(Config { dest_path, new_dir, volume_path, excluded_directories })
    } 
}

pub fn backup() -> Result<(), Box<dyn std::error::Error>> {
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
        backup_status =  run().unwrap_or_else(err_closure);
        println!("Starting containers...");
        handle_containers(&running_containers, "start")?;
    } else {
        backup_status = run().unwrap_or_else(err_closure);
    }

    send_notification(backup_status, format!("{}", err_message))?;
    Ok(())
}

fn run() -> Result<bool, Box<dyn std::error::Error>> {
    let config = Config::build(env::args())?;

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

fn exclude_dirs(command: &mut Command, dirs_to_exclude: &String) -> () {
    for dir in dirs_to_exclude.split(",") {
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