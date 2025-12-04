use std::collections::HashMap;
use std::{thread, time};

use crate::backup::logger::{LogLevel, Logger};

pub trait Notification {
    fn send_notification(&self) -> Result<(), Box<dyn std::error::Error>>;
}

pub struct Gotify<'a> {
    pub message: Option<String>,
    pub url: &'a String,
    pub success: bool,
    pub logger: &'a Logger,
}

pub struct Discord<'a> {
    pub message: Option<String>,
    pub url: &'a String,
    pub success: bool,
}

impl<'a> Notification for Gotify<'a> {
    fn send_notification(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut map: HashMap<&str, &str> = HashMap::new();

        let message = if let Some(msg) = &self.message {
            msg
        } else if self.success {
            "Backup successful"
        } else {
            "Backup failed"
        };

        map.insert("title", "Backup result");
        map.insert("message", message);
        let client = reqwest::blocking::Client::new();

        for attempt in 0..10 {
            self.logger.log(
                &format!("Sending request to Gotify.Attempt {}", attempt),
                LogLevel::Info,
            );
            let _req = client
                .post(self.url)
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .json(&map)
                .send();
            if let Ok(response) = _req {
                if response.status().is_success() {
                    return Ok(());
                }
            }
            thread::sleep(time::Duration::from_secs(10));
        }
        Err(Box::from(
            "Error sending request to gotify after 10 attempts",
        ))
    }
}

impl<'a> Notification for Discord<'a> {
    fn send_notification(&self) -> Result<(), Box<dyn std::error::Error>> {
        let status_field = format!(
            r#"{{
            "name": "Status",
            "value": "{}"
        }}"#,
            if self.success { "Success" } else { "Failed" }
        );
        let error_message_field = format!(
            r#",{{
            "name": "Message",
            "value": "{}"
        }}"#,
            if let Some(msg) = &self.message {
                msg
            } else {
                "No message"
            }
        );
        let json = format!(
            r#"
        {{
            "embeds": [
                {{
                    "title": "Docker backup result",
                    "fields": [
                        {}
                        {}
                    ]
                }}
            ]
        }}
    "#,
            status_field, error_message_field
        );
        let client = reqwest::blocking::Client::new();
        let _req = client
            .post(self.url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .body(json)
            .send();
        if _req?.status().is_success() {
            Ok(())
        } else {
            Err(Box::from("Error sending notification to discord"))
        }
    }
}

pub fn send_notification<T: Notification>(
    notification: T,
) -> Result<(), Box<dyn std::error::Error>> {
    notification.send_notification()
}
