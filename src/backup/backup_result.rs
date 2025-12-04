use std::{
    fmt::{self},
    string::FromUtf8Error,
};

use crate::backup::{
    logger::LogLevel,
    notification::{send_notification, Discord, Gotify},
};

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
        if let Some(gotify_url) = &config.gotify_url {
            send_notification::<Gotify>(Gotify {
                message: Some(format!("Backup failed with error: {}", self.message)),
                success: false,
                url: gotify_url,
                logger: &config.logger,
            })
            .unwrap_or_else(|e| {
                config.logger.log(
                    &format!("Error sending gotify notification: {}", e),
                    LogLevel::Error,
                );
            });
        }

        if let Some(dc_url) = &config.discord_url {
            send_notification::<Discord>(Discord {
                message: Some(self.message.to_string()),
                success: false,
                url: dc_url,
            })
            .unwrap_or_else(|e| {
                config.logger.log(
                    &format!("Error sending discord notification: {}", e),
                    LogLevel::Error,
                );
            });
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

pub struct BackupSuccess {
    message: String,
}

impl BackupSuccess {
    pub fn new(message: &str) -> Self {
        BackupSuccess {
            message: message.to_string(),
        }
    }
    pub fn notify(&self, config: &DockerBackup) {
        if let Some(gotify_url) = &config.gotify_url {
            send_notification::<Gotify>(Gotify {
                message: Some(self.message.clone()),
                success: true,
                url: gotify_url,
                logger: &config.logger,
            })
            .unwrap_or_else(|e| {
                config.logger.log(
                    &format!("Error sending gotify notification: {}", e),
                    LogLevel::Error,
                );
            });
        }

        if let Some(dc_url) = &config.discord_url {
            send_notification::<Discord>(Discord {
                message: Some(self.message.clone()),
                success: true,
                url: dc_url,
            })
            .unwrap_or_else(|e| {
                config.logger.log(
                    &format!("Error sending discord notification: {}", e),
                    LogLevel::Error,
                );
            });
        }
    }
}
