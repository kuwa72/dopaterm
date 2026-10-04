# dopaterm

GPU terminal emulator with input/output visual effects (dopairb-inspired), in Rust.

## Architecture

- `alacritty_terminal` crate provides PTY (ConPTY on Windows / posix openpty), escape-sequence parsing, and the IO event-loop thread (`event_loop::EventLoop` + `Msg` channel for input/resize). `Term` is shared via `Arc<FairMutex<_>>` and locked in `build_frame` only.
- Renderer (`src/render.rs`): wgpu 30 + glyphon 0.12. One instanced-quad pipeline draws cell backgrounds AND effect particles (`fx::Instance`). glyphon draws text between them. Colors authored in sRGB are converted to linear (`colors::srgb_to_linear`) because the surface is `*Srgb`.
- Effect layer (`src/fx/`): `FxEvent` (Key/Erased/CursorMoved/PtyOutput/Bell/CommandDone/Waiting) -> `Effect` trait impls emit `Particle`s -> collected as `Instance`s each frame. Built-ins in `builtin.rs`; Lua plugins in `lua_fx.rs` loaded from `<config dir>/effects/*.lua`.
- Erase detection for shatter is a per-frame grid diff (`App::build_frame` snapshot vs. current); mass changes (>1/3 of grid) are treated as scroll/redraw and suppress shatter. Backspace/Delete additionally fire a predictive erase read from the grid at key-press time (deduped ~200ms).
- CommandDone/Waiting are heuristics driven by a 200ms `UserEvent::Wake` heartbeat: Enter (non-alt-screen) -> output -> 600ms quiet = done; >3s idle = waiting pulse every 1.5s.
- Text is cached per screen row: only rows with cell/cursor changes are re-shaped (glyphon `line_bufs`), so particle animation does not re-layout the whole grid.
- Screenshot mode `--screenshot FILE` renders offscreen + PNG readback; used for headless verification.

## Build

```sh
cargo build                                # host
cargo build --release --target x86_64-pc-windows-gnu   # Windows exe (.cargo/config.toml sets mingw linker)
scripts/build-windows.sh                   # Windows release build + copy to /mnt/c/Users/ykuwa/.local/bin + sync effects/*.lua to %APPDATA%\dopaterm\effects
scripts/build-windows.sh --debug           # dev build variant
```

## Test / verify (WSL)

```sh
XDG_RUNTIME_DIR=/mnt/wslg/runtime-dir ./target/debug/dopaterm --screenshot /tmp/shot.png
# Plain WAYLAND_DISPLAY fails on this machine (socket lives in /mnt/wslg/runtime-dir).
./target/x86_64-pc-windows-gnu/release/dopaterm.exe --screenshot /tmp/shot_win.png   # WSL interop runs the real Windows binary
```

## Windows console

- The exe is built with `windows_subsystem = "windows"`, so no console window appears on normal launch. `--debug-console` (`win_console` in `src/main.rs`) attaches to the parent console or allocates a new one; valid inherited std handles (pipes, WSL interop) must be kept — only NULL handles are rebound to CONIN$/CONOUT$.

## Windows mouse regression verification

- Run `bash scripts/test-mouse-windows.sh` from WSL. It runs Linux and Windows unit tests, builds both test utilities, and verifies real window messages (move, press, drag, release, wheel) against native Windows, WSL, and PowerShell-launched WSL processes.
- Console attachment must stay in the dedicated `--mouse-helper` subprocess: detaching the GUI process breaks console writes and can invalidate its standard handles. The helper remains attached until its input pipe closes.
- Mouse tracking modes are mutually exclusive; use `intersects(TermMode::MOUSE_MODE)`, not `contains`. Native console capture is detected with `GetConsoleMode`, since ConPTY consumes its mouse-mode escape sequences. Do not change console input modes when querying them.

## IME verification

- `bash scripts/test-ime-windows.sh` runs unit tests, Windows mouse regressions, and an offscreen screenshot of Japanese preedit text. Physical Microsoft IME candidate selection and commit still require manual confirmation.
- IME is enabled with `Window::set_ime_allowed(true)` and its candidate area follows the terminal cursor in physical pixels. Preedit uses `src/ime.rs` and the existing overlay pipeline; only committed text is written to the PTY as UTF-8. Cursor ranges from winit are UTF-8 byte offsets, not character indices.
- `DOPA_IME_DEMO=1` only affects screenshot mode, adding a sample Japanese preedit overlay. IME is disabled while settings are open.

## Font selection / overlay verification

- The F1 font selector uses the renderer's system font database (`glyphon::FontSystem` / `fontdb`), filtering `FaceInfo::monospaced`, sorting and deduplicating family names. The catalog is collected once at startup; configured unavailable/proportional fonts fall back to an installed monospace preference.
- Run `bash scripts/test-fonts-windows.sh` to verify Linux/Windows tests, actual Windows font enumeration, mouse regressions, and a settings screenshot. `DOPA_SETTINGS_DEMO=1` displays settings only in screenshot mode.
- Terminal and overlay text need separate glyphon `TextRenderer`s, both prepared before encoding render passes. Reusing one renderer before queue submission overwrites its vertex data and can destroy a buffer referenced by the terminal pass.

## New UI / input

- Settings overlay (F1) renders on top of the terminal using the same text/quads pipeline. It lets the user toggle effect intensity and each built-in effect at runtime.
- Mouse tracking is supported: cursor coordinates are converted to cell positions, and SGR / X10 mouse protocol sequences are sent to the PTY when the application enables mouse mode. Mouse wheel forwards button events in mouse mode and scrolls history otherwise.
- Terminal-side selection (`Selection` in `src/app.rs`) anchors drag ranges in buffer coordinates (row - display_offset), so it follows scroll. Shift+drag/wheel bypasses app mouse mode. `Ctrl+Shift+C` copies via `selected_text`; `Ctrl+Shift+V` pastes with bracketed-paste wrapping, saving clipboard images to a temp PNG and pasting its path (`/mnt/<drive>/...` for wsl.exe children).
- `mouse_test` binary (`cargo build --bin mouse_test`) can be run inside dopaterm to verify mouse event delivery. Set `DOPA_MOUSE_LOG` to make dopaterm log outgoing mouse protocol bytes to `/tmp/dopaterm_mouse.log`.
- On Windows, ConPTY does not translate SGR mouse sequences, so dopaterm bypasses it with `WriteConsoleInputW`: native console apps receive `MOUSE_EVENT` records, and VT bridges (wsl.exe / ssh.exe) receive the escape bytes as `KEY_EVENT` records.

## Background / grid verification

- Run `bash scripts/test-background-windows.sh` for unit tests, mouse regressions, and pixel checks of native Windows screenshots before/after scrollback. `background_test` emits colored fullwidth text; `DOPA_BACKGROUND_SCROLL_DEMO=1` scrolls history only during screenshot capture.
- Snapshot all cells including `WIDE_CHAR_SPACER` colors. Trailing wide spacers use a NUL marker which is omitted from text spans and erase particles; tab cells become spaces because the terminal grid already expands their tab stops.
- Visible grid rows use `point.line + display_offset`, including cursor coordinates. The canvas clear color must use the OSC-resolved default background, and cached background quads must be rebuilt when that default changes even if cell contents do not.
- Color queries report configured/OSC-overridden colors, not unconditional black. Text buffers disable wrapping and adjust glyph advances through letter spacing to match Unicode cell widths and the renderer's cell grid; row caching remains intact.

## Known gaps (next steps)

- No text selection / clipboard copy (OSC52 store works; Ctrl+Shift+V paste works).
- No hyperlink underlining or underline/strikethrough glyph rendering.
- OSC 133 shell-integration events (command start/finish) not yet wired — current CommandDone/Waiting events are timing heuristics and can false-trigger (long output pauses, Enter inside REPLs).
- Lua plugins can only spawn particles; no text/quad-overlay drawing API yet.
