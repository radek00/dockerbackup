use std::{
    io::{Stdout, Write},
    sync::Mutex,
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

pub struct Logger {
    stdout: Mutex<Stdout>,
}

impl Logger {
    pub fn new(stdout: Stdout) -> Self {
        Self {
            stdout: Mutex::new(stdout),
        }
    }

    pub fn log(&self, message: &str, level: LogLevel) {
        let color = match level {
            LogLevel::Info => Color::Cyan,
            LogLevel::Warning => Color::Yellow,
            LogLevel::Error => Color::Red,
            LogLevel::Success => Color::Green,
        };
        let mut stdout = self.stdout.lock().unwrap();
        execute!(
            stdout,
            SetForegroundColor(color),
            Print(message),
            Print("\n"),
            ResetColor
        )
        .unwrap();
        stdout.flush().unwrap();
    }

    pub fn log_elapsed_time(&self, timer_id: usize, message: &str, color: Color) {
        let mut stdout = self.stdout.lock().unwrap();

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

    pub fn reset_cursor_after_timers(&self, active_timers: u16) {
        let mut stdout = self.stdout.lock().unwrap();
        execute!(
            stdout,
            cursor::MoveDown(active_timers + 1),
            cursor::MoveToColumn(0),
            terminal::Clear(ClearType::FromCursorDown),
        )
        .unwrap();

        stdout.flush().unwrap();
    }

    pub fn clear_terminal(&self) {
        let mut stdout = self.stdout.lock().unwrap();
        execute!(
            stdout,
            terminal::Clear(terminal::ClearType::All),
            cursor::MoveTo(0, 0),
        )
        .unwrap();

        stdout.flush().unwrap();
    }

    pub fn hide_cursor(&self) {
        let mut stdout = self.stdout.lock().unwrap();
        execute!(stdout, Hide).unwrap();
        stdout.flush().unwrap();
    }

    pub fn show_cursor(&self) {
        let mut stdout = self.stdout.lock().unwrap();
        execute!(stdout, Show).unwrap();
        stdout.flush().unwrap();
    }
}
