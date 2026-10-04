//! Standalone terminal mouse test utility.
//! Run inside dopaterm (or any terminal) to verify mouse event delivery.
//! Prints each mouse event to stdout; press 'q' to quit.

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseEventKind, read,
};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::fs::File;
use std::io::{Write, stdout};

fn main() -> std::io::Result<()> {
    let mut log = std::env::var_os("MOUSE_TEST_LOG")
        .and_then(|p| File::create(std::path::Path::new(&p)).ok());
    let mut write_line = |s: String| {
        println!("{}", s);
        if let Some(f) = &mut log {
            let _ = writeln!(f, "{}", s);
            let _ = f.flush();
        }
    };

    enable_raw_mode()?;
    let mut stdout = stdout();
    // Enable SGR 1006 mouse tracking (buttons + any motion).
    crossterm::execute!(stdout, EnableMouseCapture)?;
    stdout.flush()?;
    eprintln!("Mouse test running. Move or click the mouse. Press 'q' to quit.");
    write_line("READY".to_string());

    loop {
        match read()? {
            Event::Key(key) => {
                if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                    break;
                }
            }
            Event::Mouse(me) => {
                let kind = match me.kind {
                    MouseEventKind::Down(b) => format!("Down({})", button_name(b)),
                    MouseEventKind::Up(b) => format!("Up({})", button_name(b)),
                    MouseEventKind::Drag(b) => format!("Drag({})", button_name(b)),
                    MouseEventKind::Moved => "Moved".to_string(),
                    MouseEventKind::ScrollUp => "ScrollUp".to_string(),
                    MouseEventKind::ScrollDown => "ScrollDown".to_string(),
                    MouseEventKind::ScrollLeft => "ScrollLeft".to_string(),
                    MouseEventKind::ScrollRight => "ScrollRight".to_string(),
                };
                write_line(format!(
                    "{} col={} row={} modifiers={:?}",
                    kind, me.column, me.row, me.modifiers
                ));
                stdout.flush()?;
            }
            _ => {}
        }
    }

    crossterm::execute!(stdout, DisableMouseCapture)?;
    stdout.flush()?;
    disable_raw_mode()?;
    Ok(())
}

fn button_name(b: crossterm::event::MouseButton) -> &'static str {
    match b {
        crossterm::event::MouseButton::Left => "Left",
        crossterm::event::MouseButton::Right => "Right",
        crossterm::event::MouseButton::Middle => "Middle",
    }
}
