use std::{
    fmt::{self},
    string::FromUtf8Error,
};

use crate::backup::notification::{send_notification, Discord, Gotify};

use super::DockerBackup;

#[derive(Debug)]
pub struct BackupError {
    pub message: String,
}

impl BackupError {
    pub fn new(message: &str) -> BackupError {
        BackupError {
            message: message.to_string(),
        }
    }
    pub fn notify(&self, config: &DockerBackup) {
        println!("{}", self.message);
        if let Some(gotify_url) = &config.gotify_url {
            send_notification::<Gotify>(Gotify {
                message: Some(format!("Error message: {}", self.message)),
                success: false,
                url: gotify_url,
            })
            .unwrap();
        }

        if let Some(dc_url) = &config.discord_url {
            send_notification::<Discord>(Discord {
                message: Some(self.message.to_string()),
                success: false,
                url: dc_url,
            })
            .unwrap();
        }
    }
}

impl fmt::Display for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for BackupError {}

impl From<std::io::Error> for BackupError {
    fn from(error: std::io::Error) -> Self {
        BackupError {
            message: error.to_string(),
        }
    }
}

impl From<FromUtf8Error> for BackupError {
    fn from(error: FromUtf8Error) -> Self {
        BackupError {
            message: error.to_string(),
        }
    }
}

impl Default for BackupError {
    fn default() -> Self {
        BackupError {
            message: "An error occurred while parsing the HTTP request".to_string(),
        }
    }
}
