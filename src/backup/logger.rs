use std::{
    io::{Stdout, Write},
    sync::{Arc, Mutex},
};

use crossterm::{
    cursor::{self, Hide, Show},
    execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{self, ClearType},
};

pub enum LogLevel {
    Info,
    Warning,
    Error,
    Success,
}

pub fn log(message: &str, level: LogLevel) {
    let color = match level {
        LogLevel::Info => Color::Cyan,
        LogLevel::Warning => Color::Yellow,
        LogLevel::Error => Color::Red,
        LogLevel::Success => Color::Green,
    };
    let mut stdout = std::io::stdout();
    execute!(
        stdout,
        SetForegroundColor(color),
        Print(message),
        Print("\n"),
        ResetColor
    )
    .unwrap()
}

pub fn log_elapsed_time(
    timer_id: usize,
    message: &String,
    stdout_mutex: &Arc<Mutex<Stdout>>,
    color: Color,
) {
    let mut stdout = stdout_mutex.lock().unwrap();

    execute!(
        stdout,
        cursor::SavePosition,
        cursor::MoveDown(timer_id as u16 + 1),
        cursor::MoveToColumn(0),
        terminal::Clear(ClearType::CurrentLine),
        SetForegroundColor(color),
        Print(message),
        ResetColor,
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
