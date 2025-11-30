use std::{
    collections::HashSet,
    fs,
    io::{Stdout, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
};

use crossterm::{
    cursor::{self, Hide, Show},
    execute,
    style::Print,
    terminal::{self, ClearType},
};

use super::{backup_result::BackupError, TargetOs};

pub fn check_docker() -> Result<(), BackupError> {
    let status = Command::new("docker").arg("--version").status()?;
    if status.success() {
        return Ok(());
    }
    Err(BackupError::new("Can't continue without Docker installed"))
}

pub fn check_running_containers() -> Result<String, BackupError> {
    let running_containers = Command::new("docker")
        .args(["ps", "--format", "{{.Names}}"])
        .output()?;
    let containers_list = String::from_utf8(running_containers.stdout)?;
    Ok(containers_list)
}

pub fn exclude_volumes(
    command: &mut Command,
    dirs_to_exclude: &Vec<String>,
    volume_path: &PathBuf,
) -> Result<(), BackupError> {
    let volumes: HashSet<String> = fs::read_dir(volume_path)
        .map_err(|e| BackupError::new(&format!("Failed to read volume directory: {}", e)))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_str().map(|s| s.to_string()))
        .collect();

    for volume in dirs_to_exclude {
        if !volumes.iter().any(|x| x.ends_with(volume)) {
            return Err(BackupError::new(&format!(
                "Excluded volume '{}' does not exist",
                volume
            )));
        }
        command.arg(format!("--exclude={}", volume));
    }
    Ok(())
}

pub fn create_new_dir(dest_path: &Path, new_dir: &String) -> Result<(), BackupError> {
    let dir_path = dest_path.join(new_dir);
    if dir_path.exists() {
        return Err(BackupError::new("Directory already exists"));
    }
    std::fs::create_dir_all(dir_path)?;
    Ok(())
}

pub fn handle_containers(containers: &HashSet<&str>, command: &str) -> Result<(), BackupError> {
    let cmd_result = Command::new("docker")
        .arg(command)
        .args(containers)
        .status()?;
    if cmd_result.success() {
        return Ok(());
    }
    Err(BackupError::new("Error handling containers"))
}

pub fn parse_destination_path(path: &str) -> Result<(String, TargetOs), String> {
    if path.contains('@') {
        let tuple: Vec<&str> = path.splitn(2, ',').collect();
        if tuple.len() != 2 {
            return Err(String::from(
                "Destination path and target os must be provided",
            ));
        }

        let parts: Vec<&str> = tuple[0].splitn(2, ':').collect();
        if parts.len() == 2 && parts[0].contains('@') {
            Ok((tuple[0].to_owned(), TargetOs::from_str(tuple[1])?))
        } else {
            Err(String::from(
                "SSH path must be in the format user@host:path",
            ))
        }
    } else if Path::new(path).exists() {
        Ok((path.to_owned(), TargetOs::Windows))
    } else {
        Err(String::from("Local path does not exist"))
    }
}

pub fn get_volumes_size(
    volume_path: &PathBuf,
    excluded_volumes: &Vec<String>,
) -> Result<u64, BackupError> {
    let mut total_size = 0;
    let entries = fs::read_dir(volume_path)
        .map_err(|e| BackupError::new(&format!("Failed to read volume directory: {}", e)))?;

    for entry in entries {
        let entry =
            entry.map_err(|e| BackupError::new(&format!("Failed to read entry: {}", e)))?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if excluded_volumes.contains(&name.to_string()) {
            continue;
        }

        total_size += get_dir_size(&path).map_err(|e| {
            BackupError::new(&format!("Failed to calculate size for {}: {}", name, e))
        })?;
    }
    Ok(total_size)
}

fn get_dir_size(path: &Path) -> std::io::Result<u64> {
    let mut size = 0;
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                size += get_dir_size(&path)?;
            } else {
                size += entry.metadata()?.len();
            }
        }
    } else {
        size = path.metadata()?.len();
    }
    Ok(size)
}

pub fn check_available_space(
    dest: &(String, TargetOs),
    required_size: u64,
) -> Result<(), BackupError> {
    let available_space = if dest.0.contains('@') {
        let parts: Vec<&str> = dest.0.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(BackupError::new("Invalid ssh path"));
        }
        check_ssh_available_space(parts[0], parts[1], &dest.1)?
    } else {
        check_local_available_space(&dest.0)?
    };

    if available_space < required_size {
        return Err(BackupError::new(&format!(
            "Not enough space on destination {}. Required: {} bytes, Available: {} bytes",
            dest.0, required_size, available_space
        )));
    }
    Ok(())
}

fn check_local_available_space(path: &str) -> Result<u64, BackupError> {
    let output = Command::new("df")
        .arg("-B1")
        .arg("--output=avail")
        .arg(path)
        .output()
        .map_err(|e| BackupError::new(&format!("Failed to execute df: {}", e)))?;

    if !output.status.success() {
        return Err(BackupError::new(&format!(
            "df command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.len() < 2 {
        return Err(BackupError::new("Invalid df output"));
    }

    lines[1]
        .trim()
        .parse::<u64>()
        .map_err(|_| BackupError::new("Failed to parse available space"))
}

fn check_ssh_available_space(
    host: &str,
    path: &str,
    target_os: &TargetOs,
) -> Result<u64, BackupError> {
    match target_os {
        TargetOs::Unix => {
            let output = Command::new("ssh")
                .arg(host)
                .arg("df")
                .arg("-B1")
                .arg("--output=avail")
                .arg(path)
                .output()
                .map_err(|e| BackupError::new(&format!("Failed to execute ssh: {}", e)))?;

            if !output.status.success() {
                // Fallback for df implementations that don't support --output
                let output = Command::new("ssh")
                    .arg(host)
                    .arg("df")
                    .arg("-k")
                    .arg(path)
                    .output()
                    .map_err(|e| BackupError::new(&format!("Failed to execute ssh: {}", e)))?;

                if !output.status.success() {
                    return Err(BackupError::new(&format!(
                        "ssh df command failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }
                let stdout = String::from_utf8_lossy(&output.stdout);
                let lines: Vec<&str> = stdout.lines().collect();
                if lines.len() < 2 {
                    return Err(BackupError::new("Invalid df output"));
                }
                // Parse 4th column
                let parts: Vec<&str> = lines[1].split_whitespace().collect();
                if parts.len() < 4 {
                    return Err(BackupError::new("Invalid df output format"));
                }
                let kbytes = parts[3]
                    .parse::<u64>()
                    .map_err(|_| BackupError::new("Failed to parse available space"))?;
                return Ok(kbytes * 1024);
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = stdout.lines().collect();
            if lines.len() < 2 {
                return Err(BackupError::new("Invalid df output"));
            }

            lines[1]
                .trim()
                .parse::<u64>()
                .map_err(|_| BackupError::new("Failed to parse available space"))
        }
        TargetOs::Windows => {
            let ps_command = format!(
                "powershell -Command \"Get-Volume -FilePath '{}' | Select-Object -ExpandProperty SizeRemaining\"",
                path
            );

            let output = Command::new("ssh")
                .arg(host)
                .arg(ps_command)
                .output()
                .map_err(|e| BackupError::new(&format!("Failed to execute ssh: {}", e)))?;

            if !output.status.success() {
                return Err(BackupError::new(&format!(
                    "ssh powershell command failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout
                .trim()
                .parse::<u64>()
                .map_err(|_| BackupError::new("Failed to parse available space"))
        }
    }
}

pub fn get_elapsed_time(start: std::time::Instant, description: &str) -> String {
    let elapsed = start.elapsed();
    format!(
        "{}: {:02}:{:02}:{:02}",
        description,
        elapsed.as_secs() / 3600,
        elapsed.as_secs() % 3600 / 60,
        elapsed.as_secs() % 60
    )
}

pub fn print_elapsed_time(timer_id: usize, message: &String, stdout_mutex: &Arc<Mutex<Stdout>>) {
    let mut stdout = stdout_mutex.lock().unwrap();

    execute!(
        stdout,
        cursor::SavePosition,
        cursor::MoveDown(timer_id as u16 + 1),
        cursor::MoveToColumn(0),
        terminal::Clear(ClearType::CurrentLine),
        Print(message),
        cursor::RestorePosition,
    )
    .unwrap();

    stdout.flush().unwrap();
}

pub fn reset_cursor_after_timers(active_timers: u16, stdout: &Arc<Mutex<Stdout>>) {
    let mut stdout = stdout.lock().unwrap();
    execute!(
        stdout,
        cursor::MoveDown(active_timers + 1),
        cursor::MoveToColumn(0),
        terminal::Clear(ClearType::FromCursorDown),
    )
    .unwrap();

    stdout.flush().unwrap();
}

pub fn clear_terminal(stdout: &Arc<Mutex<Stdout>>) {
    let mut stdout = stdout.lock().unwrap();
    execute!(
        stdout,
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0),
    )
    .unwrap();

    stdout.flush().unwrap();
}

pub fn hide_cursor(stdout: &Arc<Mutex<Stdout>>) {
    let mut stdout = stdout.lock().unwrap();
    execute!(stdout, Hide).unwrap();
    stdout.flush().unwrap();
}

pub fn show_cursor(stdout: &Arc<Mutex<Stdout>>) {
    let mut stdout = stdout.lock().unwrap();
    execute!(stdout, Show).unwrap();
    stdout.flush().unwrap();
}
