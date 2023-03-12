use std::process::{self, Stdio };
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

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::build(env::args()).expect("Failed building config struct");

    check_docker();
    let containers = check_running_containers().expect("Couldn't check for running containers");
    let mut running_containers: Vec<&str> = containers.trim().split("\n").collect();
    running_containers.retain(|&x| !x.is_empty());

    if running_containers.len() == 0 {
        println!("No running containers found");
    } else {
        println!("Stopping containers...");
        handle_containers(&running_containers, "stop").expect("Failed to stop containers");
    }

    let backup_status = if config.dest_path.contains("@") {
        ssh_backup(&config)
    } else {
        local_rsync_backup(&config)
    }.unwrap_or_else(| err | {
        println!("{}", err);
        false
    });

    if running_containers.len() > 0 { 
        println!("Starting containers...");
        handle_containers(&running_containers, "start")?;
    }

    send_notification(backup_status)?;
    Ok(())
}


fn check_docker() -> () {

    let status = Command::new("docker").arg("--version").status().expect("Docker command failed to start.");
    if status.success() {
        ()
    } else {
        eprintln!("Can't continue without docker installed");
        process::exit(1);
    }
    
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

    let exec_rsync = rsync.arg("-az").arg(&config.volume_path).arg(format!("{}/{}", config.dest_path, config.new_dir)).status().expect("Rsync command failed to start.");
    if exec_rsync.success() { Ok(true) } else {Err(Box::from("Rsync backup failed"))}
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

    if ssh.success() { Ok(true) } else {Err(Box::from("Scp backup failed"))}
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
    if cmd_result.success() { Ok(()) } else { Err(Box::from("Failed to handle containers")) }
}

fn append_to_path(path: &str, new_dir: &String) -> String {
    if path.contains("\\") {
        format!("{}\\{}", path, new_dir)
    } else {
        format!("{}/{}", path, new_dir)
    }
}