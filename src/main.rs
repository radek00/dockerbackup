use backup::DockerBackup;

mod backup;
fn main() {
    DockerBackup::build().backup().expect("Backup failed");
}
