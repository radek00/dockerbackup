use dockerbackup;
use std::env;
fn main() {
    //println!("{:?}", check_running_containers());
    //let mut shell_session: Command = Command::new("sh");
    let args: Vec<String> = env::args().collect();

    let config = dockerbackup::Config::build(&args).unwrap();


    dockerbackup::check_docker();
    dockerbackup::check_running_containers();
}


