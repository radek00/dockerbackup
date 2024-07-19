mod backup;
fn main() {
    backup::backup().expect("Backup failed");
}
