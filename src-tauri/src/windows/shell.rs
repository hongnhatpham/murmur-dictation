use std::mem::size_of;

use serde::{Deserialize, Serialize};
use windows::Win32::{
    Foundation::HWND,
    UI::{
        Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_WARNING,
            NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICONDATAW_0,
            NOTIFYICON_VERSION_4,
        },
        WindowsAndMessaging::{LoadIconW, IDI_APPLICATION},
    },
};

use crate::error::{CoreError, CoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrayState {
    Idle,
    Dictating,
    Meeting,
    Processing,
    Attention,
}

impl TrayState {
    fn tooltip(self) -> &'static str {
        match self {
            Self::Idle => "Murmur",
            Self::Dictating => "Murmur - dictating",
            Self::Meeting => "Murmur - recording meeting",
            Self::Processing => "Murmur - processing",
            Self::Attention => "Murmur - needs attention",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Information,
    Attention,
}

/// Owns a native notification-area icon. The host window receives `callback_message` and can map
/// click events to opening Murmur or accepting the current Meeting Prompt.
pub struct NativeTray {
    window: HWND,
    id: u32,
    callback_message: u32,
}

impl NativeTray {
    pub fn add(window: HWND, id: u32, callback_message: u32) -> CoreResult<Self> {
        let icon = unsafe { LoadIconW(None, IDI_APPLICATION) }
            .map_err(|error| native_error("load tray icon", error))?;
        let mut data = base_data(window, id);
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = callback_message;
        data.hIcon = icon;
        set_utf16(&mut data.szTip, TrayState::Idle.tooltip());
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
            return Err(CoreError::Unavailable("add Windows tray icon".into()));
        }
        data.Anonymous = NOTIFYICONDATAW_0 {
            uVersion: NOTIFYICON_VERSION_4,
        };
        let _ = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) };
        Ok(Self {
            window,
            id,
            callback_message,
        })
    }

    pub fn set_state(&self, state: TrayState) -> CoreResult<()> {
        let mut data = base_data(self.window, self.id);
        data.uFlags = NIF_TIP;
        data.uCallbackMessage = self.callback_message;
        set_utf16(&mut data.szTip, state.tooltip());
        shell_update(NIM_MODIFY, &data, "update Windows tray state")
    }

    pub fn notify(&self, title: &str, message: &str, kind: NotificationKind) -> CoreResult<()> {
        let mut data = base_data(self.window, self.id);
        data.uFlags = NIF_INFO;
        set_utf16(&mut data.szInfoTitle, title);
        set_utf16(&mut data.szInfo, message);
        data.dwInfoFlags = match kind {
            NotificationKind::Information => NIIF_INFO,
            NotificationKind::Attention => NIIF_WARNING,
        };
        shell_update(NIM_MODIFY, &data, "show Windows notification")
    }

    pub fn callback_message(&self) -> u32 {
        self.callback_message
    }
}

impl Drop for NativeTray {
    fn drop(&mut self) {
        let data = base_data(self.window, self.id);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }
}

fn base_data(window: HWND, id: u32) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: id,
        ..Default::default()
    }
}

fn shell_update(
    operation: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE,
    data: &NOTIFYICONDATAW,
    action: &str,
) -> CoreResult<()> {
    if unsafe { Shell_NotifyIconW(operation, data) }.as_bool() {
        Ok(())
    } else {
        Err(CoreError::Unavailable(action.into()))
    }
}

fn set_utf16<const N: usize>(target: &mut [u16; N], text: &str) {
    target.fill(0);
    for (slot, value) in target
        .iter_mut()
        .take(N.saturating_sub(1))
        .zip(text.encode_utf16())
    {
        *slot = value;
    }
}

fn native_error(action: &str, error: windows::core::Error) -> CoreError {
    CoreError::Unavailable(format!("{action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_copy_truncates_and_null_terminates() {
        let mut target = [99_u16; 4];
        set_utf16(&mut target, "abcdef");
        assert_eq!(target, ['a' as u16, 'b' as u16, 'c' as u16, 0]);
    }
}
