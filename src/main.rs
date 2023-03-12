use dockerbackup;
use notification::send_notification;

mod notification;
fn main() {
    let backup_status = dockerbackup::run().unwrap_or_else(| err | {
        println!("{}", err);
        false
    });
    send_notification(backup_status).expect("Failed to send notification")
}