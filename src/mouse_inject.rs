//! Windows Console API mouse injection.
//!
//! ConPTY does not translate VT mouse escape sequences (e.g. SGR `ESC[<...M`)
//! into MOUSE_EVENT INPUT_RECORDs, so Windows console apps and WSL/SSH bridges
//! never see them when they are written to the PTY master.
//!
//! This module bypasses ConPTY by attaching to the child process's console
//! session and writing INPUT_RECORDs directly to its input buffer via
//! WriteConsoleInputW:
//!
//! - Native Windows console apps receive MOUSE_EVENT records.
//! - VT bridges (wsl.exe, ssh.exe, ...) receive the SGR escape bytes as
//!   KEY_EVENT records, which travel on to the Linux PTY unchanged.

#[cfg(windows)]
mod win {
    use std::collections::HashMap;
    use std::io::{self, BufRead, BufReader, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
    use std::sync::{LazyLock, Mutex};

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::CreateFileW;
    use windows_sys::Win32::System::Console::{
        AttachConsole, COORD, FreeConsole, GetConsoleMode, INPUT_RECORD, INPUT_RECORD_0,
        KEY_EVENT_RECORD, KEY_EVENT_RECORD_0, MOUSE_EVENT_RECORD, SetConsoleMode,
        WriteConsoleInputW,
    };

    const GENERIC_READ: u32 = 0x80000000;
    const GENERIC_WRITE: u32 = 0x40000000;
    const FILE_SHARE_READ: u32 = 0x00000001;
    const FILE_SHARE_WRITE: u32 = 0x00000002;
    const OPEN_EXISTING: u32 = 3;
    const DETACHED_PROCESS: u32 = 0x00000008;
    const ENABLE_LINE_INPUT: u32 = 0x0002;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

    // dwButtonState
    pub const FROM_LEFT_1ST_BUTTON_PRESSED: u32 = 0x0001;
    pub const RIGHTMOST_BUTTON_PRESSED: u32 = 0x0002;
    pub const FROM_LEFT_2ND_BUTTON_PRESSED: u32 = 0x0004;

    // dwEventFlags
    pub const MOUSE_MOVED: u32 = 0x0001;
    pub const MOUSE_WHEELED: u32 = 0x0004;

    // control key state
    pub const SHIFT_PRESSED: u32 = 0x0010;
    pub const LEFT_CTRL_PRESSED: u32 = 0x0008;
    pub const RIGHT_CTRL_PRESSED: u32 = 0x0004;
    pub const LEFT_ALT_PRESSED: u32 = 0x0002;
    pub const RIGHT_ALT_PRESSED: u32 = 0x0001;

    /// Console input mode flags
    const ENABLE_MOUSE_INPUT: u32 = 0x0010;
    const ENABLE_EXTENDED_FLAGS: u32 = 0x0080;
    const ENABLE_QUICK_EDIT_MODE: u32 = 0x0040;

    fn debug_log(msg: &str) {
        if std::env::var_os("DOPA_MOUSE_DEBUG").is_none() {
            return;
        }
        use std::io::Write;
        let path = std::env::temp_dir().join("dopaterm_mouse_debug.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let _ = writeln!(f, "[{}.{}] {}", now.as_secs(), now.subsec_millis(), msg);
        }
    }

    /// Cached CONIN$ handle per child PID. The handle stays valid after
    /// FreeConsole on modern Windows. Stored as isize because raw pointers are
    /// not Send.
    static HANDLES: LazyLock<Mutex<HashMap<u32, isize>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    fn conin_handle(pid: u32) -> Option<HANDLE> {
        {
            let handles = HANDLES.lock().unwrap();
            if let Some(&h) = handles.get(&pid) {
                debug_log(&format!("conin_handle cached pid={pid} handle={h:x}"));
                return Some(h as HANDLE);
            }
        }

        unsafe {
            // Detach from our own console (if any) and attach to the child's.
            let _ = FreeConsole();
            if AttachConsole(pid) == 0 {
                let err = GetLastError();
                debug_log(&format!("AttachConsole failed pid={pid} err={err}"));
                return None;
            }
            let name: Vec<u16> = std::ffi::OsStr::new("CONIN$")
                .encode_wide()
                .chain(Some(0))
                .collect();
            let handle = CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            let err = GetLastError();
            debug_log(&format!(
                "CreateFileW CONIN$ pid={pid} handle={:p} err={err}",
                handle
            ));

            if handle.is_null() || handle as isize == -1 {
                return None;
            }

            HANDLES.lock().unwrap().insert(pid, handle as isize);
            Some(handle)
        }
    }

    fn remove_handle(pid: u32) {
        let mut handles = HANDLES.lock().unwrap();
        if let Some(h) = handles.remove(&pid) {
            unsafe {
                CloseHandle(h as HANDLE);
            }
        }
    }

    fn key_record(ch: u16) -> INPUT_RECORD {
        INPUT_RECORD {
            EventType: 1, // KEY_EVENT
            Event: INPUT_RECORD_0 {
                KeyEvent: KEY_EVENT_RECORD {
                    bKeyDown: 1,
                    wRepeatCount: 1,
                    wVirtualKeyCode: 0,
                    wVirtualScanCode: 0,
                    uChar: KEY_EVENT_RECORD_0 { UnicodeChar: ch },
                    dwControlKeyState: 0,
                },
            },
        }
    }

    fn mouse_record(
        col: i16,
        row: i16,
        button_state: u32,
        control_key_state: u32,
        event_flags: u32,
    ) -> INPUT_RECORD {
        INPUT_RECORD {
            EventType: 2, // MOUSE_EVENT
            Event: INPUT_RECORD_0 {
                MouseEvent: MOUSE_EVENT_RECORD {
                    dwMousePosition: COORD { X: col, Y: row },
                    dwButtonState: button_state,
                    dwControlKeyState: control_key_state,
                    dwEventFlags: event_flags,
                },
            },
        }
    }

    fn write_records(pid: u32, records: &[INPUT_RECORD]) -> bool {
        let handle = match conin_handle(pid) {
            Some(h) => h,
            None => {
                debug_log(&format!("write_records no handle pid={pid}"));
                return false;
            }
        };
        if records.first().is_some_and(|record| record.EventType == 2) {
            unsafe {
                // Enable mouse input in the console buffer so injected records are delivered.
                let mut mode = 0u32;
                if GetConsoleMode(handle, &mut mode) != 0 {
                    let new_mode = (mode | ENABLE_MOUSE_INPUT | ENABLE_EXTENDED_FLAGS)
                        & !ENABLE_QUICK_EDIT_MODE;
                    let set = SetConsoleMode(handle, new_mode);
                    debug_log(&format!(
                        "SetConsoleMode pid={pid} old={mode:x} new={new_mode:x} ok={set}"
                    ));
                } else {
                    debug_log(&format!(
                        "GetConsoleMode failed pid={pid} err={}",
                        GetLastError()
                    ));
                }
            }
        }
        let mut written = 0;
        let (ok, err) = unsafe {
            let ok =
                WriteConsoleInputW(handle, records.as_ptr(), records.len() as u32, &mut written)
                    != 0;
            (ok, GetLastError())
        };
        debug_log(&format!(
            "WriteConsoleInputW pid={pid} count={} written={written} ok={ok}",
            records.len()
        ));
        if !ok {
            // ERROR_INVALID_HANDLE (6) means the child/console is gone.
            debug_log(&format!("WriteConsoleInputW failed pid={pid} err={err}"));
            if err == 6 {
                remove_handle(pid);
            }
        }
        ok && written as usize == records.len()
    }

    /// Inject a native MOUSE_EVENT record into the child's console input buffer.
    fn inject_mouse_event(
        pid: u32,
        col: i16,
        row: i16,
        button_state: u32,
        control_key_state: u32,
        event_flags: u32,
    ) -> bool {
        debug_log(&format!(
            "send_mouse_event pid={pid} col={col} row={row} btn_state={button_state:x} ctrl={control_key_state:x} flags={event_flags:x}"
        ));
        let rec = mouse_record(col, row, button_state, control_key_state, event_flags);
        write_records(pid, &[rec])
    }

    /// Inject a raw VT mouse escape sequence by writing each byte as a key-down
    /// INPUT_RECORD. This reaches wsl.exe/ssh.exe as stdin bytes.
    fn inject_vt_input(pid: u32, bytes: &[u8]) -> bool {
        debug_log(&format!("send_vt_input pid={pid} len={}", bytes.len()));
        let mut records = Vec::with_capacity(bytes.len());
        for &b in bytes {
            records.push(key_record(b as u16));
        }
        write_records(pid, &records)
    }

    struct Worker {
        child: Child,
        input: ChildStdin,
        output: BufReader<ChildStdout>,
    }

    impl Worker {
        fn spawn(pid: u32) -> io::Result<Self> {
            let mut child = Command::new(std::env::current_exe()?)
                .args(["--mouse-helper", &pid.to_string()])
                .creation_flags(DETACHED_PROCESS)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?;
            let input = child.stdin.take().unwrap();
            let output = BufReader::new(child.stdout.take().unwrap());
            Ok(Self {
                child,
                input,
                output,
            })
        }

        fn request(&mut self, message: &str) -> io::Result<u32> {
            writeln!(self.input, "{message}")?;
            self.input.flush()?;
            let mut reply = String::new();
            self.output.read_line(&mut reply)?;
            reply
                .trim()
                .parse()
                .map_err(|_| io::Error::other("invalid mouse helper response"))
        }
    }

    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    static WORKERS: LazyLock<Mutex<HashMap<u32, Worker>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    fn request(pid: u32, message: &str) -> Option<u32> {
        let mut workers = WORKERS.lock().unwrap();
        if let std::collections::hash_map::Entry::Vacant(entry) = workers.entry(pid) {
            match Worker::spawn(pid) {
                Ok(worker) => {
                    entry.insert(worker);
                }
                Err(err) => {
                    debug_log(&format!("mouse helper spawn failed pid={pid}: {err}"));
                    return None;
                }
            }
        }
        match workers.get_mut(&pid).unwrap().request(message) {
            Ok(reply) => Some(reply),
            Err(err) => {
                debug_log(&format!("mouse helper request failed pid={pid}: {err}"));
                workers.remove(&pid);
                None
            }
        }
    }

    pub fn input_mode(pid: u32) -> Option<u32> {
        request(pid, "mode")
    }

    pub fn native_tracking_enabled(mode: u32) -> bool {
        mode & ENABLE_MOUSE_INPUT != 0
            && mode & (ENABLE_LINE_INPUT | ENABLE_QUICK_EDIT_MODE | ENABLE_VIRTUAL_TERMINAL_INPUT)
                == 0
    }

    pub fn uses_vt_input(pid: u32) -> bool {
        input_mode(pid).is_some_and(|mode| mode & ENABLE_VIRTUAL_TERMINAL_INPUT != 0)
    }

    pub fn send_mouse_event(
        pid: u32,
        col: i16,
        row: i16,
        button_state: u32,
        control_key_state: u32,
        event_flags: u32,
    ) -> bool {
        request(
            pid,
            &format!("mouse {col} {row} {button_state} {control_key_state} {event_flags}"),
        ) == Some(1)
    }

    pub fn send_vt_input(pid: u32, bytes: &[u8]) -> bool {
        let bytes = bytes
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        request(pid, &format!("vt {bytes}")) == Some(1)
    }

    pub fn run_helper(pid: u32) -> io::Result<()> {
        let mut input = io::stdin().lock();
        let mut output = io::stdout().lock();
        let mut line = String::new();
        while input.read_line(&mut line)? != 0 {
            let mut parts = line.split_whitespace();
            let reply = match parts.next() {
                Some("mode") => {
                    let mut mode = 0;
                    if let Some(handle) = conin_handle(pid) {
                        unsafe {
                            GetConsoleMode(handle, &mut mode);
                        }
                    }
                    mode
                }
                Some("vt") => {
                    let bytes = parts
                        .map(str::parse::<u8>)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(io::Error::other)?;
                    u32::from(inject_vt_input(pid, &bytes))
                }
                Some("mouse") => {
                    let values = parts
                        .map(str::parse::<i64>)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(io::Error::other)?;
                    if values.len() != 5 {
                        return Err(io::Error::other("invalid mouse request"));
                    }
                    u32::from(inject_mouse_event(
                        pid,
                        values[0] as i16,
                        values[1] as i16,
                        values[2] as u32,
                        values[3] as u32,
                        values[4] as u32,
                    ))
                }
                _ => return Err(io::Error::other("invalid mouse helper request")),
            };
            writeln!(output, "{reply}")?;
            output.flush()?;
            line.clear();
        }
        remove_handle(pid);
        // Detach again so the caller's console state is not affected.
        unsafe {
            FreeConsole();
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn native_capture_requires_mouse_input_without_line_or_vt_input() {
            assert!(native_tracking_enabled(
                ENABLE_MOUSE_INPUT | ENABLE_EXTENDED_FLAGS
            ));
            for mode in [
                0,
                ENABLE_MOUSE_INPUT | 2,
                ENABLE_MOUSE_INPUT | ENABLE_QUICK_EDIT_MODE,
                ENABLE_MOUSE_INPUT | 0x0200,
            ] {
                assert!(!native_tracking_enabled(mode));
            }
        }
    }
}

#[cfg(not(windows))]
mod stub {
    pub fn send_mouse_event(
        _pid: u32,
        _col: i16,
        _row: i16,
        _button_state: u32,
        _control_key_state: u32,
        _event_flags: u32,
    ) -> bool {
        false
    }
    pub fn send_vt_input(_pid: u32, _bytes: &[u8]) -> bool {
        false
    }
}

#[cfg(not(windows))]
pub use stub::*;
#[cfg(windows)]
pub use win::*;
