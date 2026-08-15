//! Global toggle hotkey. Hold-to-talk is intentionally unsupported.

use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::error::CaptureError;

/// Default toggle key advertised by the listen command.
pub const DEFAULT_HOTKEY: &str = "F8";

/// Common bindings shown in the panel. Any parseable key or `Ctrl`/`Alt`/`Shift`/`Win`
/// chord can be set; this list is not the allowlist.
pub const SUPPORTED_HOTKEYS: &[&str] = &[
    "F8",
    "F9",
    "F10",
    "Pause",
    "Insert",
    "ScrollLock",
    "Ctrl+R",
    "Ctrl+Space",
    "Ctrl+Shift+R",
    "Alt+F8",
];

/// Parsed toggle binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToggleHotkey {
    pub name: String,
    registered: HotKey,
}

/// Parses a toggle binding. Function keys, letters, digits, navigation keys,
/// and optional `Ctrl`/`Alt`/`Shift`/`Win` prefixes are accepted. Modifier-only
/// specs and unknown tokens are refused. This is still a press-to-toggle binding,
/// not push-to-talk hold.
///
/// # Errors
///
/// Returns [`CaptureError::UnsupportedHotkey`] for an unusable spec.
pub fn parse_hotkey(spec: &str) -> Result<ToggleHotkey, CaptureError> {
    let normalized = spec
        .split('+')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join("+");
    if normalized.is_empty() {
        return Err(CaptureError::UnsupportedHotkey(spec.to_owned()));
    }
    let registered = HotKey::from_str(&normalized)
        .map_err(|_| CaptureError::UnsupportedHotkey(spec.trim().to_owned()))?;
    Ok(ToggleHotkey {
        name: canonicalize(registered),
        registered,
    })
}

/// Stops the polling thread when dropped.
pub struct HotkeyGuard {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for HotkeyGuard {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts a background listener that fires on each key-down edge (no hold).
///
/// # Errors
///
/// Returns [`CaptureError::UnsupportedHotkey`] when `spec` cannot be parsed,
/// or [`CaptureError::Io`] when the listener thread cannot start.
pub fn spawn_hotkey(
    spec: &str,
    sink: tokio::sync::mpsc::UnboundedSender<()>,
) -> Result<HotkeyGuard, CaptureError> {
    let parsed = parse_hotkey(spec)?;
    let running = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&running);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = thread::Builder::new()
        .name("takegraph-capture-hotkey".into())
        .spawn(move || poll_toggle(parsed, &flag, &sink, ready_tx))
        .map_err(CaptureError::Io)?;
    ready_rx
        .recv_timeout(Duration::from_secs(2))
        .map_err(|_| CaptureError::HotkeyListener("did not start".into()))??;
    Ok(HotkeyGuard {
        running,
        thread: Some(thread),
    })
}

fn poll_toggle(
    hotkey: ToggleHotkey,
    running: &AtomicBool,
    sink: &tokio::sync::mpsc::UnboundedSender<()>,
    ready: std::sync::mpsc::Sender<Result<(), CaptureError>>,
) {
    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => manager,
        Err(error) => {
            let _ = ready.send(Err(CaptureError::HotkeyListener(error.to_string())));
            return;
        }
    };
    if let Err(error) = manager.register(hotkey.registered) {
        let _ = ready.send(Err(match error {
            global_hotkey::Error::AlreadyRegistered(_) => {
                CaptureError::HotkeyInUse(hotkey.name.clone())
            }
            other => CaptureError::HotkeyListener(other.to_string()),
        }));
        return;
    }
    let _ = ready.send(Ok(()));
    let receiver = GlobalHotKeyEvent::receiver();
    while running.load(Ordering::SeqCst) {
        pump_pending_messages();
        match receiver.try_recv() {
            Ok(event)
                if event.id == hotkey.registered.id() && event.state == HotKeyState::Pressed =>
            {
                let _ = sink.send(());
            }
            _ => thread::sleep(Duration::from_millis(16)),
        }
    }
}

/// `RegisterHotKey` delivers `WM_HOTKEY` only if this thread pumps messages.
#[cfg(windows)]
fn pump_pending_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    let mut msg = MSG::default();
    // The HWND belongs to `global-hotkey`; we only dispatch queued input.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(not(windows))]
fn pump_pending_messages() {}

fn canonicalize(hotkey: HotKey) -> String {
    let mut parts = Vec::new();
    if hotkey.mods.contains(Modifiers::CONTROL) {
        parts.push("Ctrl".to_owned());
    }
    if hotkey.mods.contains(Modifiers::ALT) {
        parts.push("Alt".to_owned());
    }
    if hotkey.mods.contains(Modifiers::SHIFT) {
        parts.push("Shift".to_owned());
    }
    if hotkey.mods.contains(Modifiers::SUPER) {
        parts.push("Win".to_owned());
    }
    parts.push(key_label(hotkey.key));
    parts.join("+")
}

fn key_label(code: Code) -> String {
    match code {
        Code::F1 => "F1",
        Code::F2 => "F2",
        Code::F3 => "F3",
        Code::F4 => "F4",
        Code::F5 => "F5",
        Code::F6 => "F6",
        Code::F7 => "F7",
        Code::F8 => "F8",
        Code::F9 => "F9",
        Code::F10 => "F10",
        Code::F11 => "F11",
        Code::F12 => "F12",
        Code::F13 => "F13",
        Code::F14 => "F14",
        Code::F15 => "F15",
        Code::F16 => "F16",
        Code::F17 => "F17",
        Code::F18 => "F18",
        Code::F19 => "F19",
        Code::F20 => "F20",
        Code::F21 => "F21",
        Code::F22 => "F22",
        Code::F23 => "F23",
        Code::F24 => "F24",
        Code::KeyA => "A",
        Code::KeyB => "B",
        Code::KeyC => "C",
        Code::KeyD => "D",
        Code::KeyE => "E",
        Code::KeyF => "F",
        Code::KeyG => "G",
        Code::KeyH => "H",
        Code::KeyI => "I",
        Code::KeyJ => "J",
        Code::KeyK => "K",
        Code::KeyL => "L",
        Code::KeyM => "M",
        Code::KeyN => "N",
        Code::KeyO => "O",
        Code::KeyP => "P",
        Code::KeyQ => "Q",
        Code::KeyR => "R",
        Code::KeyS => "S",
        Code::KeyT => "T",
        Code::KeyU => "U",
        Code::KeyV => "V",
        Code::KeyW => "W",
        Code::KeyX => "X",
        Code::KeyY => "Y",
        Code::KeyZ => "Z",
        Code::Digit0 => "0",
        Code::Digit1 => "1",
        Code::Digit2 => "2",
        Code::Digit3 => "3",
        Code::Digit4 => "4",
        Code::Digit5 => "5",
        Code::Digit6 => "6",
        Code::Digit7 => "7",
        Code::Digit8 => "8",
        Code::Digit9 => "9",
        Code::Space => "Space",
        Code::Tab => "Tab",
        Code::Enter => "Enter",
        Code::Escape => "Escape",
        Code::Backspace => "Backspace",
        Code::Delete => "Delete",
        Code::Insert => "Insert",
        Code::Home => "Home",
        Code::End => "End",
        Code::PageUp => "PageUp",
        Code::PageDown => "PageDown",
        Code::Pause => "Pause",
        Code::PrintScreen => "PrintScreen",
        Code::ScrollLock => "ScrollLock",
        Code::CapsLock => "CapsLock",
        Code::ArrowUp => "Up",
        Code::ArrowDown => "Down",
        Code::ArrowLeft => "Left",
        Code::ArrowRight => "Right",
        Code::Backquote => "`",
        Code::Minus => "-",
        Code::Equal => "=",
        Code::BracketLeft => "[",
        Code::BracketRight => "]",
        Code::Backslash => "\\",
        Code::Semicolon => ";",
        Code::Quote => "'",
        Code::Comma => ",",
        Code::Period => ".",
        Code::Slash => "/",
        other => return other.to_string(),
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_function_keys_letters_and_modifier_chords() {
        assert_eq!(parse_hotkey("f8").unwrap().name, "F8");
        assert_eq!(parse_hotkey("Pause").unwrap().name, "Pause");
        assert_eq!(parse_hotkey("ctrl+r").unwrap().name, "Ctrl+R");
        assert_eq!(
            parse_hotkey("Ctrl + Shift + Space").unwrap().name,
            "Ctrl+Shift+Space"
        );
        assert_eq!(parse_hotkey("Alt+F8").unwrap().name, "Alt+F8");
        assert!(parse_hotkey("hold").is_err());
        assert!(parse_hotkey("Shift+Ctrl").is_err());
        assert!(parse_hotkey("").is_err());
    }
}
