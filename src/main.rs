use std::process::{Command };
use dockerbackup;
fn main() {
    //println!("{:?}", check_running_containers());
    let shell_session: Command = Command::new("sh");
    dockerbackup::check_docker(shell_session);
}


