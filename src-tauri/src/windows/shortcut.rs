use std::{
    collections::HashSet,
    sync::{mpsc, Arc, Mutex, OnceLock},
    thread::{self, JoinHandle},
};

use serde::{Deserialize, Serialize};
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        Input::KeyboardAndMouse::{
            VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LWIN, VK_RCONTROL, VK_RWIN,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
            TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
            WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

use crate::error::{CoreError, CoreResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutChord {
    /// Windows virtual-key codes. Left and right modifier variants are normalized.
    pub keys: Vec<u16>,
}

impl ShortcutChord {
    pub fn ctrl_win() -> Self {
        Self {
            keys: vec![VK_CONTROL.0 as u16, VK_LWIN.0 as u16],
        }
    }

    pub fn normalized(&self) -> HashSet<u16> {
        self.keys.iter().copied().map(normalize_key).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutConfig {
    pub hold_to_talk: ShortcutChord,
    pub toggle: Option<ShortcutChord>,
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            hold_to_talk: ShortcutChord::ctrl_win(),
            toggle: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutEvent {
    BeginDictation,
    FinishDictation,
    CancelDictation,
}

#[derive(Debug)]
struct ShortcutState {
    config: ShortcutConfig,
    pressed: HashSet<u16>,
    suppressed_keys: HashSet<u16>,
    hold_active: bool,
    hold_cancelled: bool,
    toggle_active: bool,
    external_active: bool,
    sender: mpsc::Sender<ShortcutEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookDisposition {
    PassThrough,
    Suppress,
}

impl ShortcutState {
    fn key_event(&mut self, key: u16, down: bool) -> HookDisposition {
        let key = normalize_key(key);
        let release_was_suppressed = !down && self.suppressed_keys.remove(&key);
        let was_down = self.pressed.contains(&key);
        if down {
            self.pressed.insert(key);
        } else {
            self.pressed.remove(&key);
        }

        if down
            && !was_down
            && key == VK_ESCAPE.0 as u16
            && (self.hold_active || self.toggle_active || self.external_active)
        {
            self.hold_cancelled = self.hold_active;
            self.hold_active = false;
            self.toggle_active = false;
            self.external_active = false;
            let _ = self.sender.send(ShortcutEvent::CancelDictation);
            self.suppressed_keys.insert(key);
            return HookDisposition::Suppress;
        }

        let hold_matches = contains_chord(&self.pressed, &self.config.hold_to_talk);
        if self.hold_cancelled {
            if hold_matches {
                return if release_was_suppressed {
                    HookDisposition::Suppress
                } else {
                    HookDisposition::PassThrough
                };
            }
            self.hold_cancelled = false;
        }
        if hold_matches && !self.hold_active && !self.toggle_active {
            self.hold_active = true;
            let _ = self.sender.send(ShortcutEvent::BeginDictation);
        } else if self.hold_active && !hold_matches {
            self.hold_active = false;
            let _ = self.sender.send(ShortcutEvent::FinishDictation);
        }

        if down && !was_down {
            if let Some(toggle) = &self.config.toggle {
                if contains_chord(&self.pressed, toggle) {
                    self.toggle_active = !self.toggle_active;
                    let event = if self.toggle_active {
                        ShortcutEvent::BeginDictation
                    } else {
                        ShortcutEvent::FinishDictation
                    };
                    let _ = self.sender.send(event);
                }
            }
        }
        let chord_is_active = contains_chord(&self.pressed, &self.config.hold_to_talk)
            || self
                .config
                .toggle
                .as_ref()
                .is_some_and(|toggle| contains_chord(&self.pressed, toggle));
        let belongs_to_shortcut = chord_contains_key(&self.config.hold_to_talk, key)
            || self
                .config
                .toggle
                .as_ref()
                .is_some_and(|toggle| chord_contains_key(toggle, key));
        // A partial Windows-key chord cannot be hidden safely. Passing Win down but swallowing
        // the completing key leaves Windows with a plain Win press and opens Start on release.
        // Let the balanced modifier chord through; Ctrl+Win alone has no Windows action.
        let windows_chord_is_active = (contains_chord(&self.pressed, &self.config.hold_to_talk)
            && chord_contains_key(&self.config.hold_to_talk, VK_LWIN.0))
            || self.config.toggle.as_ref().is_some_and(|toggle| {
                contains_chord(&self.pressed, toggle) && chord_contains_key(toggle, VK_LWIN.0)
            });
        if down && chord_is_active && belongs_to_shortcut && !windows_chord_is_active {
            self.suppressed_keys.insert(key);
            HookDisposition::Suppress
        } else if release_was_suppressed {
            HookDisposition::Suppress
        } else {
            HookDisposition::PassThrough
        }
    }
}

fn normalize_key(key: u16) -> u16 {
    match key {
        value if value == VK_LCONTROL.0 || value == VK_RCONTROL.0 => VK_CONTROL.0,
        value if value == VK_RWIN.0 => VK_LWIN.0,
        _ => key,
    }
}

fn contains_chord(pressed: &HashSet<u16>, chord: &ShortcutChord) -> bool {
    let wanted = chord.normalized();
    !wanted.is_empty() && wanted.iter().all(|key| pressed.contains(key))
}

fn chord_contains_key(chord: &ShortcutChord, key: u16) -> bool {
    chord.normalized().contains(&key)
}

static ACTIVE_HOOK: OnceLock<Mutex<Option<Arc<Mutex<ShortcutState>>>>> = OnceLock::new();

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let message = wparam.0 as u32;
        let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        let up = message == WM_KEYUP || message == WM_SYSKEYUP;
        if down || up {
            let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            if let Ok(slot) = ACTIVE_HOOK.get_or_init(Default::default).lock() {
                if let Some(state) = slot.as_ref() {
                    if let Ok(mut state) = state.lock() {
                        if state.key_event(event.vkCode as u16, down) == HookDisposition::Suppress {
                            return LRESULT(1);
                        }
                    }
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

pub struct GlobalShortcutService {
    thread_id: u32,
    join: Option<JoinHandle<CoreResult<()>>>,
    state: Arc<Mutex<ShortcutState>>,
}

impl GlobalShortcutService {
    pub fn start(config: ShortcutConfig) -> CoreResult<(Self, mpsc::Receiver<ShortcutEvent>)> {
        validate_config(&config)?;
        let (event_tx, event_rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(ShortcutState {
            config,
            pressed: HashSet::new(),
            suppressed_keys: HashSet::new(),
            hold_active: false,
            hold_cancelled: false,
            toggle_active: false,
            external_active: false,
            sender: event_tx,
        }));
        let hook_state = Arc::clone(&state);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<u32>(1);
        let join = thread::Builder::new()
            .name("murmur-shortcuts".into())
            .spawn(move || unsafe {
                let thread_id = GetCurrentThreadId();
                let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0)
                    .map_err(|error| native_error("register global keyboard hook", error))?;
                *ACTIVE_HOOK
                    .get_or_init(Default::default)
                    .lock()
                    .map_err(|_| {
                        CoreError::Unavailable("shortcut hook lock is poisoned".into())
                    })? = Some(hook_state);
                let _ = ready_tx.send(thread_id);
                let result = run_message_loop();
                let _ = UnhookWindowsHookEx(hook);
                if let Ok(mut slot) = ACTIVE_HOOK.get_or_init(Default::default).lock() {
                    *slot = None;
                }
                result
            })
            .map_err(CoreError::Io)?;
        let thread_id = ready_rx
            .recv()
            .map_err(|_| CoreError::Unavailable("shortcut thread exited during startup".into()))?;
        Ok((
            Self {
                thread_id,
                join: Some(join),
                state,
            },
            event_rx,
        ))
    }

    pub fn set_external_dictation_active(&self, active: bool) -> CoreResult<()> {
        self.state
            .lock()
            .map_err(|_| CoreError::Unavailable("shortcut state lock is poisoned".into()))?
            .external_active = active;
        Ok(())
    }

    pub fn stop(mut self) -> CoreResult<()> {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0))
                .map_err(|error| native_error("stop shortcut hook", error))?;
        }
        self.join
            .take()
            .expect("shortcut join handle exists")
            .join()
            .map_err(|_| CoreError::Unavailable("shortcut thread panicked".into()))?
    }
}

fn validate_config(config: &ShortcutConfig) -> CoreResult<()> {
    let hold = config.hold_to_talk.normalized();
    if hold.is_empty() {
        return Err(CoreError::InvalidInput(
            "hold-to-talk shortcut cannot be empty".into(),
        ));
    }
    if let Some(toggle) = &config.toggle {
        let toggle = toggle.normalized();
        if toggle.is_empty() {
            return Err(CoreError::InvalidInput(
                "toggle shortcut cannot be empty".into(),
            ));
        }
        if toggle == hold {
            return Err(CoreError::InvalidInput(
                "hold-to-talk and toggle shortcuts must be different".into(),
            ));
        }
    }
    Ok(())
}

impl Drop for GlobalShortcutService {
    fn drop(&mut self) {
        if self.join.is_some() {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

unsafe fn run_message_loop() -> CoreResult<()> {
    let mut message = MSG::default();
    loop {
        let status = GetMessageW(&mut message, None, 0, 0).0;
        if status == -1 {
            return Err(CoreError::Unavailable(
                "Windows shortcut message loop failed".into(),
            ));
        }
        if status == 0 {
            return Ok(());
        }
        let _ = TranslateMessage(&message);
        DispatchMessageW(&message);
    }
}

fn native_error(action: &str, error: windows::core::Error) -> CoreError {
    CoreError::Unavailable(format!("{action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(config: ShortcutConfig) -> (ShortcutState, mpsc::Receiver<ShortcutEvent>) {
        let (sender, receiver) = mpsc::channel();
        (
            ShortcutState {
                config,
                pressed: HashSet::new(),
                suppressed_keys: HashSet::new(),
                hold_active: false,
                hold_cancelled: false,
                toggle_active: false,
                external_active: false,
                sender,
            },
            receiver,
        )
    }

    #[test]
    fn ctrl_win_starts_and_release_finishes() {
        let (mut state, receiver) = state(ShortcutConfig::default());
        state.key_event(VK_LCONTROL.0 as u16, true);
        state.key_event(VK_RWIN.0 as u16, true);
        assert_eq!(receiver.recv().unwrap(), ShortcutEvent::BeginDictation);
        state.key_event(VK_LCONTROL.0 as u16, false);
        assert_eq!(receiver.recv().unwrap(), ShortcutEvent::FinishDictation);
    }

    #[test]
    fn repeated_keydown_does_not_retrigger_toggle() {
        let chord = ShortcutChord { keys: vec![65] };
        let (mut state, receiver) = state(ShortcutConfig {
            hold_to_talk: ShortcutChord { keys: vec![66] },
            toggle: Some(chord),
        });
        state.key_event(65, true);
        state.key_event(65, true);
        assert_eq!(receiver.recv().unwrap(), ShortcutEvent::BeginDictation);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn escape_cancels_active_dictation() {
        let (mut state, receiver) = state(ShortcutConfig::default());
        state.key_event(VK_CONTROL.0 as u16, true);
        state.key_event(VK_LWIN.0 as u16, true);
        assert_eq!(receiver.recv().unwrap(), ShortcutEvent::BeginDictation);
        state.key_event(VK_ESCAPE.0 as u16, true);
        assert_eq!(receiver.recv().unwrap(), ShortcutEvent::CancelDictation);
    }

    #[test]
    fn escape_release_does_not_restart_while_hold_chord_remains_down() {
        let (mut state, receiver) = state(ShortcutConfig::default());
        state.key_event(VK_CONTROL.0, true);
        state.key_event(VK_LWIN.0, true);
        assert_eq!(receiver.recv().unwrap(), ShortcutEvent::BeginDictation);

        state.key_event(VK_ESCAPE.0, true);
        assert_eq!(receiver.recv().unwrap(), ShortcutEvent::CancelDictation);
        state.key_event(VK_ESCAPE.0, false);

        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn escape_cancels_dictation_started_outside_the_shortcut() {
        let (mut state, receiver) = state(ShortcutConfig::default());
        state.external_active = true;
        assert_eq!(
            state.key_event(VK_ESCAPE.0 as u16, true),
            HookDisposition::Suppress
        );
        assert_eq!(receiver.recv().unwrap(), ShortcutEvent::CancelDictation);
    }

    #[test]
    fn ctrl_win_chord_passes_a_balanced_combination_to_windows() {
        let (mut state, _receiver) = state(ShortcutConfig::default());
        assert_eq!(
            state.key_event(VK_CONTROL.0, true),
            HookDisposition::PassThrough
        );
        assert_eq!(
            state.key_event(VK_LWIN.0, true),
            HookDisposition::PassThrough
        );
        assert_eq!(
            state.key_event(VK_CONTROL.0, false),
            HookDisposition::PassThrough
        );
        assert_eq!(
            state.key_event(VK_LWIN.0, false),
            HookDisposition::PassThrough
        );
    }

    #[test]
    fn ordinary_keys_pass_through_to_windows() {
        let (mut state, _receiver) = state(ShortcutConfig::default());
        assert_eq!(state.key_event(65, true), HookDisposition::PassThrough);
        assert_eq!(state.key_event(65, false), HookDisposition::PassThrough);
    }

    #[test]
    fn win_first_chord_passes_a_balanced_combination_to_windows() {
        let (mut state, receiver) = state(ShortcutConfig::default());
        assert_eq!(
            state.key_event(VK_LWIN.0, true),
            HookDisposition::PassThrough
        );
        assert_eq!(
            state.key_event(VK_CONTROL.0, true),
            HookDisposition::PassThrough
        );
        assert_eq!(receiver.recv().unwrap(), ShortcutEvent::BeginDictation);
        assert_eq!(
            state.key_event(VK_CONTROL.0, false),
            HookDisposition::PassThrough
        );
        assert_eq!(receiver.recv().unwrap(), ShortcutEvent::FinishDictation);
        assert_eq!(
            state.key_event(VK_LWIN.0, false),
            HookDisposition::PassThrough
        );
    }

    #[test]
    fn rejects_one_chord_for_both_shortcut_modes() {
        let chord = ShortcutChord::ctrl_win();
        let error = validate_config(&ShortcutConfig {
            hold_to_talk: chord.clone(),
            toggle: Some(chord),
        })
        .unwrap_err();
        assert!(error.to_string().contains("must be different"));
    }
}
