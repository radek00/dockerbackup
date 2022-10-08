use dockerbackup;
fn main() {
    //println!("{:?}", check_running_containers());
    //let mut shell_session: Command = Command::new("sh");
    dockerbackup::check_docker();
    dockerbackup::check_running_containers();
}


