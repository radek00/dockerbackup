use backup::{build_command, DockerBackup, DockerRestore};

mod backup;
fn main() {
    let matches = build_command().get_matches();
    match matches.subcommand() {
        Some(("restore", sub_matches)) => {
            DockerRestore::build(sub_matches)
                .restore()
                .expect("Restore failed");
        }
        _ => {
            DockerBackup::build(matches)
                .backup()
                .expect("Backup failed");
        }
    }
}
