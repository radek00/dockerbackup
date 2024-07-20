use backup::DockerBackup;

mod backup;
fn main() {
    DockerBackup::build().backup_volumes();
}
