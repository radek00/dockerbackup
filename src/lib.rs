use std::process;
use std::process:: { Command };
use std::env;
use chrono::{self, Datelike};

pub struct Config {
    pub dest_path: String,
    pub new_dir: String,
    //pub ssh_dest: String,
    pub volume_path: String,
    pub excluded_directories: String,
}

impl Config {
    pub fn build(args: &[String]) -> Result<Config, &'static str> {
        if args.len() < 2 {
            return Err("not enough arguments");
        }
        
        let date = chrono::Local::now();

        let dest_path = args[1].clone();
        let new_dir = format!("{}-{}-{}", date.year(), date.month(), date.day());
        //let ssh_dest = args[2].clone();
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
        stop_containers(&running_containers);
    }
    local_rsync_backup(&config);
    if running_containers.len() > 0 { start_containers(&running_containers) } 
    ()
}


fn check_docker() -> () {

    let status = Command::new("docker").arg("--version").status().unwrap_or_else(| err | {
        eprintln!("Error executing command: {}", err);
        process::exit(1);
    });
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

fn local_rsync_backup(config: &Config) -> () {
    create_new_dir(&config);
    let mut rsync = Command::new("rsync");
    for dir in config.excluded_directories.split(",") {
        rsync.arg(format!("--exclude={}", dir));
    }
    let exec_rsync = rsync.arg("-az").arg(&config.volume_path).arg(format!("{}/{}", config.dest_path, config.new_dir)).status().unwrap_or_else(| err | {
        eprint!("Error executing rsync comand: {}", err);
        process::exit(1);
    });

    if exec_rsync.success() {println!("Backup successful")} else { eprintln!("Backup failed") };

    
}

fn create_new_dir(config: &Config) -> () {
    let new_dir = Command::new("mkdir").arg("-p").arg(format!("{}/{}", config.dest_path, config.new_dir)).status().unwrap_or_else(| err | {
        eprintln!("Error creating directory: {}", err);
        process::exit(1);
    });
    if new_dir.success() { () } else { process::exit(1) }

}

fn start_containers(containers: &Vec<&str>) -> () {
    println!("Starting containers...");
    let started = Command::new("docker").arg("start").args(containers).status().unwrap();
    if started.success() {println!("Containers started.")} else { panic!("Couldn't start containers.") };
}

fn stop_containers(containers: &Vec<&str>) -> () {
    println!("Stoppig containers...");
    let status = Command::new("docker").arg("stop").args(containers).status().unwrap();
    if status.success()  {println!("Containers stopped.")} else { panic!("Couldn't stop containers.") };
}