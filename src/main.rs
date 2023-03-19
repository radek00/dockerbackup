use dockerbackup;
fn main() {
    dockerbackup::backup().expect("Backup failed");
}