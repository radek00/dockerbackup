use std::process;
use std::process:: { Command };
use std::env;
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
    pub fn build(args: &[String]) -> Result<Config, &'static str> {
        if args.len() < 3 {
            return Err("not enough arguments");
        }
        
        let date = chrono::Local::now();

        let dest_path = args[1].clone();
        let new_dir = format!("{}-{}-{}", date.year(), date.month(), date.day());
        let volume_path = args[2].clone();
        let excluded_directories;
        if args.get(3).is_none() {
            excluded_directories = String::new();
        } else {
            excluded_directories = args[3].clone();
        }


        Ok(Config { dest_path, new_dir, volume_path, excluded_directories })
    } 
}

pub fn run() -> () {
    let args: Vec<String> = env::args().collect();
    let config = Config::build(&args).unwrap();

    check_docker();
    let containers = check_running_containers();
    let mut running_containers: Vec<&str> = containers.trim().split("\n").collect();
    running_containers.retain(|&x| !x.is_empty());

    if running_containers.len() == 0 {
        println!("No running containers found");
    } else {
        println!("Stopping containers...");
        handle_containers(&running_containers, "stop").expect("Failed to stop containers");
    }

    let backup_status = local_rsync_backup(&config).unwrap_or_else(| err | {
        println!("{}", err);
        false
    });

    if running_containers.len() > 0 { 
        println!("Starting containers...");
        handle_containers(&running_containers, "start").unwrap_or_else(| err | {
        println!("{}", err);
    })}

    send_notification(backup_status).unwrap_or_else(| err | {
        println!("{}", err);
    });
    ()
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

fn check_running_containers() -> String {
    let running_containers = Command::new("docker").args(["container", "ls", "-q"]).output().unwrap();
    let containers_list = String::from_utf8(running_containers.stdout).unwrap();
    containers_list
}

fn local_rsync_backup(config: &Config) -> Result<bool, Box<dyn std::error::Error>> {
    if let None = create_new_dir(&config) {
        return Err(Box::from("Could not create directory"))
    } 

    let mut rsync = Command::new("rsync");
    for dir in config.excluded_directories.split(",") {
        rsync.arg(format!("--exclude={}", dir));
    }
    let exec_rsync = rsync.arg("-az").arg(&config.volume_path).arg(format!("{}/{}", config.dest_path, config.new_dir)).status().expect("Rsync command failed to start.");

    if exec_rsync.success() { Ok(true) } else {Err(Box::from("Rsync backup failed"))}
}

fn create_new_dir(config: &Config) -> Option<()> {
    let new_dir = Command::new("mkdir").arg("-p").arg(format!("{}/{}", config.dest_path, config.new_dir)).status().expect("Mkdir command failed to start.");
    if new_dir.success() { Some(()) } else { None }

}

fn handle_containers(containers: &Vec<&str>, command: &str) -> Result<(), Box<dyn std::error::Error>> {
    let cmd_result = Command::new("docker").arg(command).args(containers).status().unwrap();
    if cmd_result.success() { Ok(()) } else { Err(Box::from("Failed to handle containers")) }

    
}