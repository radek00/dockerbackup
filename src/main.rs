use dockerbackup;
fn main() {
    match dockerbackup::run()  {
        Ok(()) => (),
        Err(err) => {
            println!("{}", err);
        }
    }
}