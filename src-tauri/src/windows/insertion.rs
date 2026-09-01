use std::{mem::size_of, ptr, thread, time::Duration};

use windows::{
    core::BSTR,
    Win32::{
        Foundation::HANDLE,
        System::{
            Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT},
            Ole::{OleInitialize, OleUninitialize, CF_UNICODETEXT},
        },
        UI::{
            Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationValuePattern,
                TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, UIA_TextPatternId,
                UIA_ValuePatternId,
            },
            Input::KeyboardAndMouse::{
                SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL,
                VK_V,
            },
        },
    },
};

use crate::{
    adapters::TextInsertion,
    error::{CoreError, CoreResult},
};

use super::context::insertion_target_allowed;

pub struct WindowsTextInsertion;

impl WindowsTextInsertion {
    pub fn new() -> Self {
        Self
    }

    /// Confirms that the focused control exposes the marker after an insertion check.
    pub fn focused_text_contains(&self, marker: &str) -> CoreResult<bool> {
        let marker = marker.to_string();
        thread::Builder::new()
            .name("murmur-insertion-verify".into())
            .spawn(move || focused_text_contains_on_sta(&marker))
            .map_err(CoreError::Io)?
            .join()
            .map_err(|_| CoreError::Unavailable("insertion verification worker panicked".into()))?
    }
}

impl Default for WindowsTextInsertion {
    fn default() -> Self {
        Self::new()
    }
}

impl TextInsertion for WindowsTextInsertion {
    fn insert_accessibly(&self, text: &str) -> CoreResult<()> {
        let text = text.to_string();
        thread::Builder::new()
            .name("murmur-accessibility-insert".into())
            .spawn(move || insert_with_ui_automation(&text))
            .map_err(CoreError::Io)?
            .join()
            .map_err(|_| CoreError::Unavailable("accessibility insertion worker panicked".into()))?
    }

    fn paste_retaining_clipboard(&self, text: &str) -> CoreResult<()> {
        let text = text.to_string();
        thread::Builder::new()
            .name("murmur-clipboard-paste".into())
            .spawn(move || paste_on_sta(&text))
            .map_err(CoreError::Io)?
            .join()
            .map_err(|_| CoreError::Unavailable("clipboard paste worker panicked".into()))?
    }
}

fn insert_with_ui_automation(text: &str) -> CoreResult<()> {
    require_allowed_target()?;
    let _ole = OleApartment::initialize()?;
    let automation: IUIAutomation = unsafe {
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| native_error("create UI Automation client", error))?
    };
    let focused = unsafe {
        automation
            .GetFocusedElement()
            .map_err(|error| native_error("read focused accessibility element", error))?
    };
    let value = unsafe {
        focused
            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .map_err(|error| native_error("focused field has no writable value pattern", error))?
    };
    if unsafe { value.CurrentIsReadOnly().unwrap_or(true.into()).as_bool() } {
        return Err(CoreError::Unavailable(
            "focused accessibility field is read-only".into(),
        ));
    }
    let current = unsafe { value.CurrentValue() }
        .map_err(|error| native_error("read focused accessibility value", error))?
        .to_string();
    let replacement = if current.is_empty() {
        text.to_string()
    } else {
        accessible_replacement(&focused, text)?
    };
    unsafe { value.SetValue(&BSTR::from(&replacement)) }
        .map_err(|error| native_error("set focused accessibility value", error))?;
    let inserted = unsafe { value.CurrentValue() }
        .map_err(|error| native_error("verify focused accessibility value", error))?;
    if inserted.to_string() == replacement {
        Ok(())
    } else {
        Err(CoreError::Unavailable(
            "focused field did not accept accessibility insertion".into(),
        ))
    }
}

fn focused_text_contains_on_sta(marker: &str) -> CoreResult<bool> {
    let _ole = OleApartment::initialize()?;
    let automation: IUIAutomation = unsafe {
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| native_error("create UI Automation client", error))?
    };
    let focused = unsafe {
        automation
            .GetFocusedElement()
            .map_err(|error| native_error("read focused accessibility element", error))?
    };
    if let Ok(value) =
        unsafe { focused.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
    {
        let current = unsafe { value.CurrentValue() }
            .map_err(|error| native_error("read focused accessibility value", error))?;
        return Ok(current.to_string().contains(marker));
    }
    let pattern = unsafe {
        focused
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .map_err(|error| native_error("focused field exposes no readable text", error))?
    };
    let document = unsafe { pattern.DocumentRange() }
        .map_err(|error| native_error("read focused document range", error))?;
    let current = unsafe { document.GetText(-1) }
        .map_err(|error| native_error("read focused document text", error))?;
    Ok(current.to_string().contains(marker))
}

fn accessible_replacement(
    focused: &windows::Win32::UI::Accessibility::IUIAutomationElement,
    text: &str,
) -> CoreResult<String> {
    unsafe {
        let pattern = focused
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .map_err(|error| native_error("focused field has no text selection pattern", error))?;
        let ranges = pattern
            .GetSelection()
            .map_err(|error| native_error("read focused text selection", error))?;
        if ranges.Length().unwrap_or_default() != 1 {
            return Err(CoreError::Unavailable(
                "focused field does not expose one text selection".into(),
            ));
        }
        let selection = ranges
            .GetElement(0)
            .map_err(|error| native_error("read focused text range", error))?;
        let document = pattern
            .DocumentRange()
            .map_err(|error| native_error("read focused document range", error))?;
        let prefix = document
            .Clone()
            .map_err(|error| native_error("clone accessibility prefix range", error))?;
        prefix
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_End,
                &selection,
                TextPatternRangeEndpoint_Start,
            )
            .map_err(|error| native_error("bound accessibility prefix range", error))?;
        let suffix = document
            .Clone()
            .map_err(|error| native_error("clone accessibility suffix range", error))?;
        suffix
            .MoveEndpointByRange(
                TextPatternRangeEndpoint_Start,
                &selection,
                TextPatternRangeEndpoint_End,
            )
            .map_err(|error| native_error("bound accessibility suffix range", error))?;
        let prefix = prefix
            .GetText(-1)
            .map_err(|error| native_error("read accessibility prefix", error))?;
        let suffix = suffix
            .GetText(-1)
            .map_err(|error| native_error("read accessibility suffix", error))?;
        Ok(format!(
            "{}{text}{}",
            prefix.to_string(),
            suffix.to_string()
        ))
    }
}

fn paste_on_sta(text: &str) -> CoreResult<()> {
    run_paste_transaction(
        || set_unicode_clipboard(text),
        || {
            require_allowed_target()?;
            let paste = shortcut_inputs(VK_CONTROL.0, VK_V.0);
            send_inputs(&paste, "paste text")?;
            thread::sleep(Duration::from_millis(100));
            Ok(())
        },
    )
}

/// Installs the Dictation Result before attempting paste and leaves it available afterward.
fn run_paste_transaction(
    retain_requested: impl FnOnce() -> CoreResult<()>,
    attempt_paste: impl FnOnce() -> CoreResult<()>,
) -> CoreResult<()> {
    retain_requested()?;
    attempt_paste()
}

fn require_allowed_target() -> CoreResult<()> {
    if insertion_target_allowed()? {
        Ok(())
    } else {
        Err(CoreError::Unavailable(
            "insertion is blocked for this sensitive target".into(),
        ))
    }
}

fn shortcut_inputs(modifier: u16, key: u16) -> [INPUT; 4] {
    [
        keyboard_input(modifier, 0, Default::default()),
        keyboard_input(key, 0, Default::default()),
        keyboard_input(key, 0, KEYEVENTF_KEYUP),
        keyboard_input(modifier, 0, KEYEVENTF_KEYUP),
    ]
}

fn keyboard_input(
    virtual_key: u16,
    scan_code: u16,
    flags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS,
) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(virtual_key),
                wScan: scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_inputs(inputs: &[INPUT], action: &str) -> CoreResult<()> {
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent == inputs.len() as u32 {
        Ok(())
    } else {
        Err(CoreError::Unavailable(format!(
            "{action}: Windows accepted {sent} of {} input events",
            inputs.len()
        )))
    }
}

fn set_unicode_clipboard(text: &str) -> CoreResult<()> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let bytes = wide.len() * size_of::<u16>();
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes)
            .map_err(|error| native_error("allocate clipboard text", error))?;
        let destination = GlobalLock(memory);
        if destination.is_null() {
            return Err(CoreError::Unavailable(
                "lock clipboard text allocation".into(),
            ));
        }
        ptr::copy_nonoverlapping(wide.as_ptr().cast::<u8>(), destination.cast::<u8>(), bytes);
        let _ = GlobalUnlock(memory);

        let _clipboard = ClipboardGuard::open()?;
        EmptyClipboard().map_err(|error| native_error("empty clipboard", error))?;
        SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(memory.0)))
            .map_err(|error| native_error("set clipboard text", error))?;
        // SetClipboardData now owns the HGLOBAL.
        Ok(())
    }
}

struct ClipboardGuard;

impl ClipboardGuard {
    fn open() -> CoreResult<Self> {
        let mut last_error = None;
        for _ in 0..10 {
            match unsafe { OpenClipboard(None) } {
                Ok(()) => return Ok(Self),
                Err(error) => {
                    last_error = Some(error);
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
        Err(native_error(
            "open clipboard",
            last_error.expect("clipboard open attempted"),
        ))
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

struct OleApartment;

impl OleApartment {
    fn initialize() -> CoreResult<Self> {
        unsafe { OleInitialize(None) }
            .map_err(|error| native_error("initialize OLE clipboard access", error))?;
        Ok(Self)
    }
}

impl Drop for OleApartment {
    fn drop(&mut self) {
        unsafe { OleUninitialize() }
    }
}

fn native_error(action: &str, error: windows::core::Error) -> CoreError {
    CoreError::Unavailable(format!("{action}: {error}"))
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    #[test]
    fn paste_shortcut_releases_key_and_modifier() {
        let inputs = shortcut_inputs(VK_CONTROL.0, VK_V.0);
        unsafe {
            assert_eq!(inputs[0].Anonymous.ki.wVk, VK_CONTROL);
            assert_eq!(inputs[1].Anonymous.ki.wVk, VK_V);
            assert_eq!(inputs[2].Anonymous.ki.dwFlags, KEYEVENTF_KEYUP);
            assert_eq!(inputs[3].Anonymous.ki.dwFlags, KEYEVENTF_KEYUP);
        }
    }

    #[test]
    fn failed_paste_retains_requested_text_without_restoring_previous_clipboard() {
        let actions = Rc::new(RefCell::new(Vec::new()));
        let result = run_paste_transaction(
            {
                let actions = Rc::clone(&actions);
                move || {
                    actions.borrow_mut().push("retain requested");
                    Ok(())
                }
            },
            {
                let actions = Rc::clone(&actions);
                move || {
                    actions.borrow_mut().push("reject sensitive target");
                    Err(CoreError::Unavailable("sensitive target".into()))
                }
            },
        );
        assert!(result.is_err());
        assert_eq!(
            actions.borrow().as_slice(),
            ["retain requested", "reject sensitive target"]
        );
    }

    #[test]
    fn successful_paste_keeps_requested_text_on_clipboard() {
        let actions = Rc::new(RefCell::new(Vec::new()));
        let result = run_paste_transaction(
            {
                let actions = Rc::clone(&actions);
                move || {
                    actions.borrow_mut().push("retain requested");
                    Ok(())
                }
            },
            {
                let actions = Rc::clone(&actions);
                move || {
                    actions.borrow_mut().push("paste");
                    Ok(())
                }
            },
        );
        assert!(result.is_ok());
        assert_eq!(actions.borrow().as_slice(), ["retain requested", "paste"]);
    }
}
