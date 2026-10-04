use std::io::{self, Write};
use std::time::Duration;

fn main() -> io::Result<()> {
    let mut output = io::stdout().lock();
    output.write_all(b"\x1b[?25l\x1b[48;2;38;42;49m\x1b[38;2;197;200;198m\x1b[2J\x1b[H")?;
    for row in 0..100 {
        write!(output, "\x1b[2K日本語 ABC {row:03} 背景色\r\n")?;
    }
    output.flush()?;
    std::thread::sleep(Duration::from_secs(30));
    Ok(())
}
