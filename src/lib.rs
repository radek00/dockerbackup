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
    let running_containers = check_running_containers();
    local_rsync_backup(&config);
    if start_containers(running_containers) {
        println!("Containers started successfully");
    } else {
        eprintln!("Couldn't start containers")
    }
    ()
}


fn check_docker() -> () {

    let status = create_shell_session().arg("docker --version").status().unwrap_or_else(| err | {
        panic!("Error executing command: {}", err)
    });
    if status.success() {
        ()
    } else {
        eprintln!("Can't continue without docker installed");
        process::exit(1);
    }

    
}

fn check_running_containers() -> String {
    let running_containers = create_shell_session().arg("docker container ls -q").output().unwrap();

    let containers_list = String::from_utf8(running_containers.stdout).unwrap().replace("\n", " ");

    if containers_list.is_empty() {
        println!("No running containers found");
    } else { 
        stop_containers(&containers_list);
    }
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

    // let rsync = Command::new("rsync").args([format!("--exclude={{{}}}", config.excluded_directories).as_str(), "-az", config.volume_path.as_str(), format!("{}/{}", config.dest_path, config.new_dir).as_str()]).status().unwrap_or_else(| err | {
    //     eprint!("Error executing rsync comand: {}", err);
    //     process::exit(1);
    // });

    if exec_rsync.success() {println!("Backup successful")} else { eprintln!("Backup failed") };

    
}

fn create_new_dir(config: &Config) -> () {
    let new_dir = create_shell_session().arg(format!("mkdir -p {}/{}", config.dest_path, config.new_dir)).status().unwrap_or_else(| err | {
        eprintln!("Error creating directory: {}", err);
        process::exit(1);
    });
    if new_dir.success() { () } else { process::exit(1) }

}

fn start_containers(containers: String) -> bool {
    let started = create_shell_session().arg(format!("docker start {}", containers)).status().unwrap();
    started.success()
}

fn stop_containers(containers: &String) -> () {
    println!("Stoppig containers...");
    let status = create_shell_session().arg(format!("docker stop {}", containers)).status().unwrap();
    if status.success()  {println!("Containers stopped.")} else { panic!("Couldn't stop containers.") };
}

fn create_shell_session() -> Command {
    let mut shell =  Command::new("sh");
    shell.arg("-c");
    return shell;
}