use std::{collections::HashMap};


pub fn send_notification(success: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut map: HashMap<&str, String> = HashMap::new();
    let message;
    if success {
        message = String::from("Backup successful");
    } else {
        message = String::from("Backup failed");
    }
    map.insert("title", String::from("Backup result"));
    map.insert("message", message);
    //println!("{:#?}",map);
    let client = reqwest::blocking::Client::new();
    let _req = client.post("https://gotify.radekserver.xyz/message?token=AOAga4xZ8pQ5c9")
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&map)
            .send();
    if _req.unwrap().status().is_success() {
        Ok(())
    } else { Err(Box::from("Error sending request to gotify"))}
 
}