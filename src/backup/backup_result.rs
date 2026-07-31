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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_error_message() {
        let err = BackupError::new("something went wrong");
        assert_eq!(err.to_string(), "something went wrong");
    }

    #[test]
    fn converts_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err: BackupError = io_err.into();
        assert_eq!(err.message, "file missing");
    }

    #[test]
    fn converts_from_utf8_error() {
        let invalid_utf8 = vec![0, 159, 146, 150];
        let utf8_err = String::from_utf8(invalid_utf8).unwrap_err();
        let err: BackupError = utf8_err.into();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn default_error_has_fallback_message() {
        let err = BackupError::default();
        assert_eq!(
            err.message,
            "An error occurred while parsing the HTTP request"
        );
    }

    #[test]
    fn backup_success_stores_message() {
        let success = BackupSuccess::new("Backup to destination /tmp completed");
        assert_eq!(success.message, "Backup to destination /tmp completed");
    }
}
