use std::process;
use std::process:: { Command };


pub fn check_docker(mut shell_session: Command) -> () {

    let status = shell_session.arg("-c").arg("docker --version").status().unwrap_or_else(| err | {
        panic!("Error executing command: {}", err)
    });

    if status.success() {
        ()
    } else {
        eprintln!("Can't continue without docker installed");
        process::exit(1);
    }

    
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