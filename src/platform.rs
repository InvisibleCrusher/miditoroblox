//! Operating-system keyboard output and the Right Alt monitor.
use crate::runtime::{SharedState, activate_panic};
use crate::solver::KeyCode;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(target_os = "linux")]
pub use evdev::uinput::VirtualDevice;
#[cfg(target_os = "windows")]
pub struct VirtualDevice;

#[cfg(target_os = "linux")]
pub fn build_keyboard() -> std::io::Result<VirtualDevice> {
    use evdev::AttributeSet;
    let mut keys = AttributeSet::<KeyCode>::new();
    for key in [
        KeyCode::KEY_LEFTSHIFT,
        KeyCode::KEY_LEFTCTRL,
        KeyCode::KEY_LEFTALT,
        KeyCode::KEY_RIGHTALT,
        KeyCode::KEY_UP,
        KeyCode::KEY_DOWN,
        KeyCode::KEY_APOSTROPHE,
    ]
    .into_iter()
    .chain(
        crate::solver::get_available_mappings()
            .iter()
            .map(|m| m.key_code),
    )
    .chain(VELOCITY_KEYS.into_iter().map(crate::solver::key_from_code))
    {
        keys.insert(key);
    }
    VirtualDevice::builder()?
        .name("Miditoroblox Rust Presser")
        .with_keys(&keys)?
        .build()
}
#[cfg(target_os = "windows")]
pub fn build_keyboard() -> std::io::Result<VirtualDevice> {
    Ok(VirtualDevice)
}

#[cfg(target_os = "linux")]
pub fn send_key(device: &mut VirtualDevice, key: KeyCode, down: bool) -> std::io::Result<()> {
    use evdev::{EventType, InputEvent};
    device.emit(&[InputEvent::new(
        EventType::KEY.0,
        key.code(),
        i32::from(down),
    )])
}
#[cfg(target_os = "windows")]
pub fn send_key(_: &mut VirtualDevice, key: KeyCode, down: bool) -> std::io::Result<()> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
        KEYEVENTF_SCANCODE, SendInput,
    };
    let scan = vk_to_scan(key.code()).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Unsupported keyboard key")
    })?;
    let mut flags = KEYEVENTF_SCANCODE;
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    if matches!(key.code(), 0x26 | 0x28 | 0x25 | 0x27 | 0xA3 | 0xA5) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT {
        wVk: 0,
        wScan: scan,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    };
    if unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) } != 1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn vk_to_scan(vk: u16) -> Option<u16> {
    Some(match vk {
        0x41 => 0x1E,
        0x42 => 0x30,
        0x43 => 0x2E,
        0x44 => 0x20,
        0x45 => 0x12,
        0x46 => 0x21,
        0x47 => 0x22,
        0x48 => 0x23,
        0x49 => 0x17,
        0x4A => 0x24,
        0x4B => 0x25,
        0x4C => 0x26,
        0x4D => 0x32,
        0x4E => 0x31,
        0x4F => 0x18,
        0x50 => 0x19,
        0x51 => 0x10,
        0x52 => 0x13,
        0x53 => 0x1F,
        0x54 => 0x14,
        0x55 => 0x16,
        0x56 => 0x2F,
        0x57 => 0x11,
        0x58 => 0x2D,
        0x59 => 0x15,
        0x5A => 0x2C,
        0x30 => 0x0B,
        0x31 => 0x02,
        0x32 => 0x03,
        0x33 => 0x04,
        0x34 => 0x05,
        0x35 => 0x06,
        0x36 => 0x07,
        0x37 => 0x08,
        0x38 => 0x09,
        0x39 => 0x0A,
        0xA0 => 0x2A,
        0xA1 => 0x36,
        0xA2 | 0xA3 => 0x1D,
        0xA4 | 0xA5 => 0x38,
        0x26 => 0x48,
        0x28 => 0x50,
        0x25 => 0x4B,
        0x27 => 0x4D,
        0xDE => 0x28,
        _ => return None,
    })
}

fn manual_key(shared: &SharedState, code: u16, down: bool, right_alt: &mut bool) {
    if code == KeyCode::KEY_RIGHTALT.code() {
        if down && !*right_alt && shared.panic_enabled.load(Ordering::Relaxed) {
            activate_panic(shared);
        }
        *right_alt = down;
    }
    if *right_alt
        && down
        && VELOCITY_KEYS.contains(&code)
        && shared.control_velocity_enabled.load(Ordering::Relaxed)
    {
        // A hook callback must never wait for the output worker's lock.
        if let Ok(mut state) = shared.device_state.try_lock() {
            let key = crate::solver::key_from_code(code);
            let result = send_key(&mut state.device, key, true)
                .and_then(|_| send_key(&mut state.device, key, false));
            state.pipeline.last_velocity = None;
            if let Err(e) = result {
                shared.report_error(format!("Manual velocity output failed: {e}"));
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub fn spawn_panic_monitor(shared: Arc<SharedState>) -> JoinHandle<()> {
    thread::spawn(move || {
        use evdev::{Device, EventType};
        let mut devices = Vec::<(std::path::PathBuf, Device, bool)>::new();
        let mut next_scan = std::time::Instant::now();
        let mut reported = false;
        while !shared.shutdown.load(Ordering::SeqCst) {
            if std::time::Instant::now() >= next_scan {
                match std::fs::read_dir("/dev/input") {
                    Ok(entries) => {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if !path
                                .file_name()
                                .is_some_and(|n| n.to_string_lossy().starts_with("event"))
                                || devices.iter().any(|(p, _, _)| *p == path)
                            {
                                continue;
                            }
                            let Ok(device) = Device::open(&path) else {
                                continue;
                            };
                            if device.name() == Some("Miditoroblox Rust Presser")
                                || !device
                                    .supported_keys()
                                    .is_some_and(|k| k.contains(KeyCode::KEY_RIGHTALT))
                            {
                                continue;
                            }
                            if device.set_nonblocking(true).is_ok() {
                                devices.push((path, device, false));
                            }
                        }
                        if devices.is_empty() && !reported {
                            shared.report_error(
                                "Right Alt monitor unavailable: no readable keyboard in /dev/input"
                                    .into(),
                            );
                            reported = true;
                        }
                    }
                    Err(e) if !reported => {
                        shared.report_error(format!("Right Alt monitor unavailable: {e}"));
                        reported = true;
                    }
                    _ => {}
                }
                next_scan = std::time::Instant::now() + Duration::from_secs(1);
            }
            devices.retain_mut(|(_, device, right_alt)| match device.fetch_events() {
                Ok(events) => {
                    for ev in events {
                        if ev.event_type() == EventType::KEY && ev.value() != 2 {
                            manual_key(&shared, ev.code(), ev.value() != 0, right_alt);
                        }
                    }
                    true
                }
                Err(e) => {
                    e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted
                }
            });
            thread::sleep(Duration::from_millis(5));
        }
    })
}

#[cfg(target_os = "windows")]
pub fn spawn_panic_monitor(shared: Arc<SharedState>) -> JoinHandle<()> {
    thread::spawn(move || {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CallNextHookEx, HC_ACTION, KBDLLHOOKSTRUCT, MSG, PM_REMOVE, PeekMessageW,
            SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
        };
        thread_local! {
            static SHARED: std::cell::RefCell<Option<Arc<SharedState>>> = const { std::cell::RefCell::new(None) };
            static ALT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        SHARED.with(|s| *s.borrow_mut() = Some(shared.clone()));
        unsafe extern "system" fn hook(code: i32, wparam: usize, lparam: isize) -> isize {
            if code == HC_ACTION as i32 {
                let ev = unsafe { *(lparam as *const KBDLLHOOKSTRUCT) };
                if ev.flags & 0x10 == 0 {
                    SHARED.with(|s| {
                        if let Some(shared) = s.borrow().as_ref() {
                            let mut alt = ALT.with(|a| a.get());
                            manual_key(
                                shared,
                                ev.vkCode as u16,
                                wparam == WM_KEYDOWN as usize || wparam == WM_SYSKEYDOWN as usize,
                                &mut alt,
                            );
                            ALT.with(|a| a.set(alt));
                        }
                    });
                }
            }
            unsafe { CallNextHookEx(0, code, wparam, lparam) }
        }
        unsafe {
            let handle = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), 0, 0);
            if handle == 0 {
                shared.report_error(format!(
                    "Right Alt monitor unavailable: {}",
                    std::io::Error::last_os_error()
                ));
                return;
            }
            while !shared.shutdown.load(Ordering::SeqCst) {
                let mut message: MSG = std::mem::zeroed();
                while PeekMessageW(&mut message, 0, 0, 0, PM_REMOVE) != 0 {}
                thread::sleep(Duration::from_millis(5));
            }
            UnhookWindowsHookEx(handle);
        }
        SHARED.with(|s| *s.borrow_mut() = None);
    })
}

#[cfg(target_os = "linux")]
pub const VELOCITY_KEYS: [u16; 32] = [
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 30, 31, 32, 33, 34, 35,
    36, 37, 38, 44, 45, 46,
];
#[cfg(target_os = "windows")]
pub const VELOCITY_KEYS: [u16; 32] = [
    0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x30, 0x51, 0x57, 0x45, 0x52, 0x54, 0x59,
    0x55, 0x49, 0x4f, 0x50, 0x41, 0x53, 0x44, 0x46, 0x47, 0x48, 0x4a, 0x4b, 0x4c, 0x5a, 0x58, 0x43,
];
