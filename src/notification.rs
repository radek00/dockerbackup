use std::collections::HashMap;
use std::{thread, time};

pub fn send_notification(success: bool, msg: String) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", msg);
    let mut map: HashMap<&str, String> = HashMap::new();
    let message;
    if success {
        message = String::from("Backup successful");
    } else {
        message = format!("Backup failed \n Error message: {}", msg);
    }
    map.insert("title", String::from("Backup result"));
    map.insert("message", message);
    thread::sleep(time::Duration::from_secs(60));
    let client = reqwest::blocking::Client::new();
    let _req = client.post("https://gotify.radekserver.xyz/message?token=AOAga4xZ8pQ5c9Y")
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&map)
            .send();
    if _req?.status().is_success() {
        Ok(())
    } else { Err(Box::from("Error sending request to gotify"))}
 
}