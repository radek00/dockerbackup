use std::process;
use std::process:: { Command };

pub struct Config {
    pub dest_path: String,
    pub new_dir: String,
    pub ssh_dest: String
}

impl Config {
    pub fn build(args: &[String]) -> Result<Config, &'static str> {
        if args.len() < 2 {
            return Err("not enough arguments");
        }
        
        //let date = SystemTime::now();

        let dest_path = args[1].clone();
        let new_dir = String::from("dh");
        let ssh_dest = args[2].clone();

        Ok(Config { dest_path, new_dir, ssh_dest })
    } 
}


pub fn check_docker() -> () {

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

pub fn check_running_containers() -> () {
    let running_containers = create_shell_session().arg("docker container ls -q").output().unwrap();

    let test = String::from_utf8(running_containers.stdout).unwrap();

    if test.is_empty() {
        println!("No running containers found");
        ()
    } else { stop_containers(test) }
}

fn stop_containers(containers: String) -> () {
    println!("Stoppig containers...");
    let status = create_shell_session().arg(format!("docker stop {}", containers)).status().unwrap();
    if status.success()  {println!("Containers stopped.")} else { panic!("Couldn't stop containers.") };
}

fn create_shell_session() -> Command {
    let mut shell =  Command::new("sh");
    shell.arg("-c");
    return shell;
}

//pub

// pub fn check_running_containers() -> bool {
//     //let from_shell = Command::new("sh").arg("-c").arg("docker container ls -q").output().expect("failed");
//     //return String::from_utf8(from_shell.status).unwrap();
//     // let running_containers = shell_session.arg("docker container ls -q").output().unwrap_or_else(| err | {
//     //     eprintln!("Error running docker command: {}", err);
//     //     panic!("Can't continue ")
//     // });
// }