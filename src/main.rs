mod backup;
fn main() {
    backup::Config::build().backup().expect("Backup failed");
}
