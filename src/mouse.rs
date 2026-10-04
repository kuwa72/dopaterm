//! Terminal mouse protocol encoder. Currently SGR 1006 and legacy X10.

use alacritty_terminal::term::TermMode;

/// Encode a mouse event into bytes to send to the PTY.
/// `col` and `line` are 0-based cell coordinates and are converted to 1-based
/// in the protocol output.
pub fn encode(
    mode: &TermMode,
    mut button: u8,
    col: usize,
    line: usize,
    release: bool,
    shift: bool,
    alt: bool,
    ctrl: bool,
) -> Option<Vec<u8>> {
    let mouse_enabled = tracking_enabled(*mode);
    if !mouse_enabled {
        return None;
    }

    if shift {
        button |= 4;
    }
    if alt {
        button |= 8;
    }
    if ctrl {
        button |= 16;
    }

    let col = col.saturating_add(1);
    let line = line.saturating_add(1);

    if mode.contains(TermMode::SGR_MOUSE) {
        let trailer = if release { 'm' } else { 'M' };
        Some(format!("\x1b[<{button};{col};{line}{trailer}").into_bytes())
    } else {
        // Legacy X10 encoding. Release is always encoded as button 3.
        if release {
            button = 3;
        }
        let cb = (button as u32 + 32).min(255) as u8;
        let cx = (col as u32 + 32).min(255) as u8;
        let cy = (line as u32 + 32).min(255) as u8;
        Some(vec![0x1b, b'[', b'M', cb, cx, cy])
    }
}

pub fn tracking_enabled(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE)
}

pub fn is_vt_bridge(program: &str) -> bool {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    matches!(
        name.to_ascii_lowercase().as_str(),
        "wsl" | "wsl.exe" | "ssh" | "ssh.exe"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sgr_mode() -> TermMode {
        let mut m = TermMode::default();
        m.insert(TermMode::MOUSE_REPORT_CLICK);
        m.insert(TermMode::SGR_MOUSE);
        m
    }

    fn x10_mode() -> TermMode {
        let mut m = TermMode::default();
        m.insert(TermMode::MOUSE_REPORT_CLICK);
        m
    }

    fn motion_mode() -> TermMode {
        let mut m = TermMode::default();
        m.insert(TermMode::MOUSE_MOTION);
        m.insert(TermMode::SGR_MOUSE);
        m
    }

    #[test]
    fn sgr_left_press() {
        let bytes = encode(&sgr_mode(), 0, 0, 0, false, false, false, false).unwrap();
        assert_eq!(bytes, b"\x1b[<0;1;1M");
    }

    #[test]
    fn sgr_left_release() {
        let bytes = encode(&sgr_mode(), 0, 1, 2, true, false, false, false).unwrap();
        assert_eq!(bytes, b"\x1b[<0;2;3m");
    }

    #[test]
    fn sgr_right_press_with_ctrl() {
        let bytes = encode(&sgr_mode(), 2, 9, 4, false, false, false, true).unwrap();
        assert_eq!(bytes, b"\x1b[<18;10;5M");
    }

    #[test]
    fn sgr_motion_no_button() {
        let bytes = encode(&motion_mode(), 35, 4, 5, false, false, false, false).unwrap();
        assert_eq!(bytes, b"\x1b[<35;5;6M");
    }

    #[test]
    fn sgr_wheel_down() {
        let bytes = encode(&sgr_mode(), 65, 2, 3, false, false, false, false).unwrap();
        assert_eq!(bytes, b"\x1b[<65;3;4M");
    }

    #[test]
    fn x10_left_press() {
        let bytes = encode(&x10_mode(), 0, 0, 0, false, false, false, false).unwrap();
        assert_eq!(bytes, &[0x1b, b'[', b'M', 32, 33, 33]);
    }

    #[test]
    fn x10_release() {
        let bytes = encode(&x10_mode(), 0, 0, 0, true, false, false, false).unwrap();
        assert_eq!(bytes, &[0x1b, b'[', b'M', 35, 33, 33]);
    }

    #[test]
    fn vt_bridge_detection_uses_executable_name() {
        for program in ["wsl", "WSL.EXE", r"C:\Windows\System32\ssh.exe"] {
            assert!(is_vt_bridge(program));
        }
        for program in [
            r"\\wsl.localhost\Ubuntu\mouse_test.exe",
            r"C:\ssh\powershell.exe",
            "ssh-agent.exe",
        ] {
            assert!(!is_vt_bridge(program));
        }
    }

    #[test]
    fn tracking_modes_are_individually_enabled() {
        for mode in [
            TermMode::MOUSE_REPORT_CLICK,
            TermMode::MOUSE_DRAG,
            TermMode::MOUSE_MOTION,
        ] {
            assert!(tracking_enabled(mode));
        }
        assert!(!tracking_enabled(TermMode::default()));
        assert!(!tracking_enabled(TermMode::SGR_MOUSE));
    }

    #[test]
    fn no_mouse_mode_returns_none() {
        let mode = TermMode::default();
        assert!(encode(&mode, 0, 0, 0, false, false, false, false).is_none());
    }
}
