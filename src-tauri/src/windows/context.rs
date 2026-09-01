use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{CloseHandle, HWND},
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
                COINIT_APARTMENTTHREADED,
            },
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
                IUIAutomationTextRange, IUIAutomationValuePattern, TextPatternRangeEndpoint_End,
                TextPatternRangeEndpoint_Start, TextUnit_Character, UIA_TextPatternId,
                UIA_ValuePatternId,
            },
            WindowsAndMessaging::{
                GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
            },
        },
    },
};

use crate::error::{CoreError, CoreResult};

const MAX_CONTEXT_CHARS: i32 = 4_000;
const CORRECTION_WINDOW: Duration = Duration::from_secs(30);
const CORRECTION_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedCorrection {
    pub heard: String,
    pub replacement: String,
}

/// Watches the focused field for at most 30 seconds. Call this on a worker thread immediately
/// after insertion. UI Automation ranges partition the document so surrounding edits invalidate
/// attribution instead of being learned as corrections.
pub fn observe_attributed_correction(heard: &str) -> CoreResult<Option<ObservedCorrection>> {
    if heard.is_empty() {
        return Ok(None);
    }
    let _com = ComApartment::initialize()?;
    let automation: IUIAutomation = unsafe {
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| native_error("create correction observer", error))?
    };
    let focused = unsafe {
        automation
            .GetFocusedElement()
            .map_err(|error| native_error("read correction target", error))?
    };
    if unsafe { focused.CurrentIsPassword().unwrap_or_default().as_bool() } {
        return Ok(None);
    }
    let ranges = match TrackedInsertion::from_focused(&focused, heard) {
        Ok(Some(ranges)) => ranges,
        Ok(None) => return Ok(None),
        Err(error) => return Err(error),
    };
    let started = Instant::now();
    let mut replacement = heard.to_string();
    while started.elapsed() < CORRECTION_WINDOW {
        thread::sleep(
            CORRECTION_POLL_INTERVAL.min(CORRECTION_WINDOW.saturating_sub(started.elapsed())),
        );
        let Some(observation) = ranges.read() else {
            return Ok(None);
        };
        match attribute_replacement(&ranges.baseline, &observation) {
            Attribution::Unchanged => replacement.clone_from(&ranges.baseline.heard),
            Attribution::Changed(value) => replacement = value,
            Attribution::Inconclusive => return Ok(None),
        }
        let current_focus = match unsafe { automation.GetFocusedElement() } {
            Ok(element) => element,
            Err(_) => break,
        };
        let same_focus = unsafe { automation.CompareElements(&focused, &current_focus) }
            .map(|same| same.as_bool())
            .unwrap_or(false);
        if !same_focus {
            break;
        }
    }
    if replacement == heard || replacement.contains(heard) {
        Ok(None)
    } else {
        Ok(Some(ObservedCorrection {
            heard: heard.to_string(),
            replacement,
        }))
    }
}

struct TrackedInsertion {
    prefix: IUIAutomationTextRange,
    inserted: IUIAutomationTextRange,
    suffix: IUIAutomationTextRange,
    document: IUIAutomationTextRange,
    baseline: RangeSnapshot,
}

impl TrackedInsertion {
    fn from_focused(element: &IUIAutomationElement, heard: &str) -> CoreResult<Option<Self>> {
        unsafe {
            let pattern =
                match element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
                    Ok(pattern) => pattern,
                    Err(_) => return Ok(None),
                };
            let selections = pattern
                .GetSelection()
                .map_err(|error| native_error("read inserted text selection", error))?;
            if selections.Length().unwrap_or_default() != 1 {
                return Ok(None);
            }
            let selection = selections
                .GetElement(0)
                .map_err(|error| native_error("read inserted text range", error))?;
            let selected = selection
                .GetText(-1)
                .map_err(|error| native_error("read selected inserted text", error))?
                .to_string();
            let inserted = selection
                .Clone()
                .map_err(|error| native_error("clone inserted text range", error))?;
            if selected != heard {
                if !selected.is_empty() {
                    return Ok(None);
                }
                let units: i32 = heard.chars().count().try_into().map_err(|_| {
                    CoreError::InvalidInput("inserted text is too long to observe".into())
                })?;
                inserted
                    .MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, -units)
                    .map_err(|error| native_error("locate inserted text range", error))?;
                if inserted
                    .GetText(-1)
                    .map_err(|error| native_error("verify inserted text range", error))?
                    .to_string()
                    != heard
                {
                    return Ok(None);
                }
            }
            let document = pattern
                .DocumentRange()
                .map_err(|error| native_error("read correction document", error))?;
            let prefix = document
                .Clone()
                .map_err(|error| native_error("clone correction prefix", error))?;
            prefix
                .MoveEndpointByRange(
                    TextPatternRangeEndpoint_End,
                    &inserted,
                    TextPatternRangeEndpoint_Start,
                )
                .map_err(|error| native_error("bound correction prefix", error))?;
            let suffix = document
                .Clone()
                .map_err(|error| native_error("clone correction suffix", error))?;
            suffix
                .MoveEndpointByRange(
                    TextPatternRangeEndpoint_Start,
                    &inserted,
                    TextPatternRangeEndpoint_End,
                )
                .map_err(|error| native_error("bound correction suffix", error))?;
            let baseline = read_ranges(&prefix, &inserted, &suffix, &document)
                .ok_or_else(|| CoreError::Unavailable("read correction ranges".into()))?;
            if baseline.heard != heard {
                return Ok(None);
            }
            Ok(Some(Self {
                prefix,
                inserted,
                suffix,
                document,
                baseline,
            }))
        }
    }

    fn read(&self) -> Option<RangeSnapshot> {
        read_ranges(&self.prefix, &self.inserted, &self.suffix, &self.document)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RangeSnapshot {
    prefix: String,
    heard: String,
    suffix: String,
    document: String,
}

fn read_ranges(
    prefix: &IUIAutomationTextRange,
    inserted: &IUIAutomationTextRange,
    suffix: &IUIAutomationTextRange,
    document: &IUIAutomationTextRange,
) -> Option<RangeSnapshot> {
    unsafe {
        Some(RangeSnapshot {
            prefix: prefix.GetText(-1).ok()?.to_string(),
            heard: inserted.GetText(-1).ok()?.to_string(),
            suffix: suffix.GetText(-1).ok()?.to_string(),
            document: document.GetText(-1).ok()?.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Attribution {
    Unchanged,
    Changed(String),
    Inconclusive,
}

fn attribute_replacement(baseline: &RangeSnapshot, current: &RangeSnapshot) -> Attribution {
    if current.prefix != baseline.prefix || current.suffix != baseline.suffix {
        return Attribution::Inconclusive;
    }
    if format!("{}{}{}", current.prefix, current.heard, current.suffix) != current.document {
        return Attribution::Inconclusive;
    }
    if current.heard == baseline.heard {
        Attribution::Unchanged
    } else if current.heard.contains(&baseline.heard) {
        // Text appended at a range boundary can be assigned to either neighboring range by a UIA
        // provider. Reject it unless the original inserted content itself changed.
        Attribution::Inconclusive
    } else {
        Attribution::Changed(current.heard.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextCapturePolicy {
    pub enabled: bool,
    pub denied_applications: Vec<String>,
}

impl Default for ContextCapturePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            denied_applications: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSnapshot {
    pub process_name: String,
    pub window_title: String,
    pub selected_text: Option<String>,
    pub surrounding_text: Option<String>,
    pub excluded_reason: Option<ContextExclusion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextExclusion {
    Disabled,
    DeniedApplication,
    PasswordField,
    PasswordManager,
    PrivateBrowserWindow,
    WindowsSecurity,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct ActiveWindow {
    pub handle: HWND,
    pub process_id: u32,
    pub process_name: String,
    pub title: String,
    pub class_name: String,
}

pub fn capture_context(policy: &ContextCapturePolicy) -> CoreResult<ContextSnapshot> {
    if !policy.enabled {
        return Ok(excluded_snapshot(ContextExclusion::Disabled));
    }
    let active = active_window()?;
    let mut snapshot = ContextSnapshot {
        process_name: active.process_name.clone(),
        window_title: active.title.clone(),
        selected_text: None,
        surrounding_text: None,
        excluded_reason: None,
    };
    if let Some(reason) = static_exclusion(&active, &policy.denied_applications) {
        return Ok(excluded_snapshot(reason));
    }

    let _com = ComApartment::initialize()?;
    let automation: IUIAutomation = unsafe {
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| native_error("create UI Automation client", error))?
    };
    let focused = unsafe {
        automation
            .GetFocusedElement()
            .map_err(|error| native_error("read focused accessibility element", error))?
    };
    if unsafe { focused.CurrentIsPassword().unwrap_or_default().as_bool() } {
        return Ok(excluded_snapshot(ContextExclusion::PasswordField));
    }

    let (selected, surrounding) = read_accessible_text(&focused);
    snapshot.selected_text = selected;
    snapshot.surrounding_text = surrounding;
    if snapshot.selected_text.is_none() && snapshot.surrounding_text.is_none() {
        snapshot.excluded_reason = Some(ContextExclusion::Unavailable);
    }
    Ok(snapshot)
}

fn excluded_snapshot(reason: ContextExclusion) -> ContextSnapshot {
    ContextSnapshot {
        process_name: String::new(),
        window_title: String::new(),
        selected_text: None,
        surrounding_text: None,
        excluded_reason: Some(reason),
    }
}

/// Applies the same hard exclusions before Murmur types or pastes into the focused target.
pub(crate) fn insertion_target_allowed() -> CoreResult<bool> {
    let active = active_window()?;
    if static_exclusion(&active, &[]).is_some() {
        return Ok(false);
    }
    let _com = ComApartment::initialize()?;
    let automation: IUIAutomation = unsafe {
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| native_error("create UI Automation client", error))?
    };
    let focused = unsafe {
        automation
            .GetFocusedElement()
            .map_err(|error| native_error("read focused accessibility element", error))?
    };
    Ok(!unsafe { focused.CurrentIsPassword().unwrap_or_default().as_bool() })
}

pub fn active_window() -> CoreResult<ActiveWindow> {
    unsafe {
        let handle = GetForegroundWindow();
        if handle.0.is_null() {
            return Err(CoreError::Unavailable("no foreground window".into()));
        }
        window_info(handle)
    }
}

pub(crate) unsafe fn window_info(handle: HWND) -> CoreResult<ActiveWindow> {
    let mut process_id = 0;
    GetWindowThreadProcessId(handle, Some(&mut process_id));
    let process_name = process_name(process_id)?;
    Ok(ActiveWindow {
        handle,
        process_id,
        process_name,
        title: window_text(handle),
        class_name: class_name(handle),
    })
}

fn read_accessible_text(focused: &IUIAutomationElement) -> (Option<String>, Option<String>) {
    unsafe {
        if let Ok(pattern) =
            focused.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
        {
            let selected = pattern
                .GetSelection()
                .ok()
                .and_then(|ranges| (ranges.Length().ok()? > 0).then_some(ranges))
                .and_then(|ranges| ranges.GetElement(0).ok())
                .and_then(|range| range.GetText(MAX_CONTEXT_CHARS).ok())
                .map(|text| text.to_string())
                .filter(|text| !text.is_empty());
            let surrounding = pattern
                .DocumentRange()
                .ok()
                .and_then(|range| range.GetText(MAX_CONTEXT_CHARS).ok())
                .map(|text| text.to_string())
                .filter(|text| !text.is_empty());
            return (selected, surrounding);
        }
        let surrounding = focused
            .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .ok()
            .and_then(|pattern| pattern.CurrentValue().ok())
            .map(|value| truncate_chars(value.to_string(), MAX_CONTEXT_CHARS as usize))
            .filter(|text| !text.is_empty());
        (None, surrounding)
    }
}

fn static_exclusion(active: &ActiveWindow, denylist: &[String]) -> Option<ContextExclusion> {
    let process = active.process_name.to_ascii_lowercase();
    let title = active.title.to_ascii_lowercase();
    let class_name = active.class_name.to_ascii_lowercase();
    if denylist.iter().any(|denied| {
        let denied = denied.to_ascii_lowercase();
        process == denied || process.trim_end_matches(".exe") == denied.trim_end_matches(".exe")
    }) {
        return Some(ContextExclusion::DeniedApplication);
    }
    if is_password_manager(&process) {
        return Some(ContextExclusion::PasswordManager);
    }
    if is_windows_security(&process, &class_name) {
        return Some(ContextExclusion::WindowsSecurity);
    }
    if is_browser(&process) && is_private_title(&title) {
        return Some(ContextExclusion::PrivateBrowserWindow);
    }
    None
}

fn is_password_manager(process: &str) -> bool {
    const NAMES: &[&str] = &[
        "1password",
        "bitwarden",
        "dashlane",
        "keepass",
        "keeper",
        "nordpass",
        "proton pass",
        "protonpass",
    ];
    NAMES.iter().any(|name| process.contains(name))
}

fn is_windows_security(process: &str, class_name: &str) -> bool {
    matches!(
        process.trim_end_matches(".exe"),
        "credentialuibroker" | "securityhealthhost" | "securityhealthsystray" | "lsass"
    ) || class_name.contains("credential dialog xaml host")
}

fn is_browser(process: &str) -> bool {
    matches!(
        process.trim_end_matches(".exe"),
        "chrome" | "msedge" | "firefox" | "brave" | "opera" | "vivaldi"
    )
}

fn is_private_title(title: &str) -> bool {
    title.contains("incognito")
        || title.contains("inprivate")
        || title.contains("private browsing")
        || title.contains("private window")
}

pub(crate) unsafe fn process_name(process_id: u32) -> CoreResult<String> {
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
        .map_err(|error| native_error("open foreground process", error))?;
    let mut buffer = vec![0_u16; 32_768];
    let mut length = buffer.len() as u32;
    let result = QueryFullProcessImageNameW(
        handle,
        PROCESS_NAME_WIN32,
        PWSTR(buffer.as_mut_ptr()),
        &mut length,
    );
    let _ = CloseHandle(handle);
    result.map_err(|error| native_error("read foreground process path", error))?;
    let path = String::from_utf16_lossy(&buffer[..length as usize]);
    Ok(Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&path)
        .to_string())
}

unsafe fn window_text(handle: HWND) -> String {
    let mut buffer = vec![0_u16; 2_048];
    let length = GetWindowTextW(handle, &mut buffer).max(0) as usize;
    String::from_utf16_lossy(&buffer[..length])
}

unsafe fn class_name(handle: HWND) -> String {
    let mut buffer = vec![0_u16; 512];
    let length = GetClassNameW(handle, &mut buffer).max(0) as usize;
    String::from_utf16_lossy(&buffer[..length])
}

fn truncate_chars(value: String, limit: usize) -> String {
    value.chars().take(limit).collect()
}

struct ComApartment(bool);

impl ComApartment {
    fn initialize() -> CoreResult<Self> {
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result.is_ok() {
            Ok(Self(true))
        } else if result.0 == 0x80010106_u32 as i32 {
            // The caller already initialized COM with another apartment model.
            Ok(Self(false))
        } else {
            Err(native_error(
                "initialize COM for accessibility",
                result.into(),
            ))
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() }
        }
    }
}

fn native_error(action: &str, error: windows::core::Error) -> CoreError {
    CoreError::Unavailable(format!("{action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(process: &str, title: &str) -> ActiveWindow {
        ActiveWindow {
            handle: HWND::default(),
            process_id: 1,
            process_name: process.into(),
            title: title.into(),
            class_name: String::new(),
        }
    }

    #[test]
    fn blocks_private_browser_but_not_normal_browser() {
        assert_eq!(
            static_exclusion(&window("msedge.exe", "InPrivate"), &[]),
            Some(ContextExclusion::PrivateBrowserWindow)
        );
        assert_eq!(
            static_exclusion(&window("msedge.exe", "Project notes"), &[]),
            None
        );
    }

    #[test]
    fn blocks_password_managers_and_user_denylist() {
        assert_eq!(
            static_exclusion(&window("Bitwarden.exe", "Vault"), &[]),
            Some(ContextExclusion::PasswordManager)
        );
        assert_eq!(
            static_exclusion(&window("Obsidian.exe", "Notes"), &["obsidian".into()]),
            Some(ContextExclusion::DeniedApplication)
        );
    }

    #[test]
    fn hard_exclusions_do_not_retain_window_metadata() {
        let snapshot = excluded_snapshot(ContextExclusion::PasswordManager);

        assert!(snapshot.process_name.is_empty());
        assert!(snapshot.window_title.is_empty());
        assert!(snapshot.selected_text.is_none());
        assert!(snapshot.surrounding_text.is_none());
        assert_eq!(
            snapshot.excluded_reason,
            Some(ContextExclusion::PasswordManager)
        );
    }

    fn snapshot(prefix: &str, heard: &str, suffix: &str) -> RangeSnapshot {
        RangeSnapshot {
            prefix: prefix.into(),
            heard: heard.into(),
            suffix: suffix.into(),
            document: format!("{prefix}{heard}{suffix}"),
        }
    }

    #[test]
    fn attributes_only_a_changed_inserted_span() {
        let baseline = snapshot("Before ", "Nhat", " after");
        let corrected = snapshot("Before ", "Nhất", " after");
        assert_eq!(
            attribute_replacement(&baseline, &corrected),
            Attribution::Changed("Nhất".into())
        );
    }

    #[test]
    fn surrounding_edits_are_never_attributed() {
        let baseline = snapshot("Before ", "Nhat", " after");
        let prefix_edit = snapshot("Changed ", "Nhat", " after");
        let suffix_edit = snapshot("Before ", "Nhat", " elsewhere");
        assert_eq!(
            attribute_replacement(&baseline, &prefix_edit),
            Attribution::Inconclusive
        );
        assert_eq!(
            attribute_replacement(&baseline, &suffix_edit),
            Attribution::Inconclusive
        );
    }

    #[test]
    fn boundary_append_is_not_learned_as_a_correction() {
        let baseline = snapshot("", "hello", "");
        let appended = snapshot("", "hello world", "");
        assert_eq!(
            attribute_replacement(&baseline, &appended),
            Attribution::Inconclusive
        );
    }

    #[test]
    fn inconsistent_provider_ranges_are_rejected() {
        let baseline = snapshot("Before ", "Nhat", " after");
        let mut inconsistent = snapshot("Before ", "Nhất", " after");
        inconsistent.document = "Different document".into();
        assert_eq!(
            attribute_replacement(&baseline, &inconsistent),
            Attribution::Inconclusive
        );
    }
}
