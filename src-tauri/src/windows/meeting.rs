use std::{collections::HashSet, time::Duration};

use serde::{Deserialize, Serialize};
use windows::core::BOOL;
use windows::Win32::{
    Foundation::{HWND, LPARAM},
    UI::WindowsAndMessaging::{EnumWindows, IsWindowVisible},
};

use crate::{error::CoreError, error::CoreResult};

use super::context::window_info;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeetingApplication {
    Zoom,
    Teams,
    GoogleMeet,
    Slack,
    Discord,
}

impl MeetingApplication {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Zoom => "Zoom",
            Self::Teams => "Microsoft Teams",
            Self::GoogleMeet => "Google Meet",
            Self::Slack => "Slack Huddle",
            Self::Discord => "Discord",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingActivity {
    pub application: MeetingApplication,
    pub process_name: String,
    pub window_title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeetingPromptEvent {
    OfferStart(MeetingActivity),
    OfferStop,
}

#[derive(Debug, Default)]
pub struct MeetingPromptTracker {
    recording: bool,
    prompted_for: Option<MeetingApplication>,
    dismissed: HashSet<MeetingApplication>,
    inactive_for: Duration,
    stop_prompted: bool,
}

impl MeetingPromptTracker {
    pub fn observe(
        &mut self,
        activity: Option<MeetingActivity>,
        microphone_active: bool,
        system_audio_active: bool,
        elapsed: Duration,
    ) -> Option<MeetingPromptEvent> {
        if !self.recording {
            let Some(activity) = activity else {
                self.prompted_for = None;
                return None;
            };
            if self.dismissed.contains(&activity.application)
                || self.prompted_for == Some(activity.application)
            {
                return None;
            }
            self.prompted_for = Some(activity.application);
            return Some(MeetingPromptEvent::OfferStart(activity));
        }

        if self.stop_prompted {
            return None;
        }
        if activity.is_some() && (microphone_active || system_audio_active) {
            self.inactive_for = Duration::ZERO;
            return None;
        }
        self.inactive_for = self.inactive_for.saturating_add(elapsed);
        if activity.is_none() || self.inactive_for >= Duration::from_secs(60) {
            self.inactive_for = Duration::ZERO;
            self.stop_prompted = true;
            return Some(MeetingPromptEvent::OfferStop);
        }
        None
    }

    pub fn set_recording(&mut self, recording: bool) {
        self.recording = recording;
        self.inactive_for = Duration::ZERO;
        self.stop_prompted = false;
        if !recording {
            self.prompted_for = None;
        }
    }

    /// Dismisses only the current call. A later call may prompt again.
    pub fn dismiss_current(&mut self, application: MeetingApplication) {
        self.prompted_for = Some(application);
    }

    /// Persists in preferences when the user chooses "Don't suggest for this app".
    pub fn suppress_application(&mut self, application: MeetingApplication) {
        self.dismissed.insert(application);
    }
}

pub fn detect_meeting_activity() -> CoreResult<Option<MeetingActivity>> {
    let mut windows = Vec::<MeetingActivity>::new();
    unsafe {
        EnumWindows(
            Some(collect_meeting_window),
            LPARAM((&mut windows as *mut Vec<MeetingActivity>) as isize),
        )
        .map_err(|error| CoreError::Unavailable(format!("enumerate meeting windows: {error}")))?;
    }
    Ok(windows.into_iter().next())
}

unsafe extern "system" fn collect_meeting_window(handle: HWND, data: LPARAM) -> BOOL {
    if !IsWindowVisible(handle).as_bool() {
        return true.into();
    }
    if let Ok(window) = window_info(handle) {
        if let Some(application) = classify_meeting(&window.process_name, &window.title) {
            let output = &mut *(data.0 as *mut Vec<MeetingActivity>);
            output.push(MeetingActivity {
                application,
                process_name: window.process_name,
                window_title: window.title,
            });
        }
    }
    true.into()
}

fn classify_meeting(process: &str, title: &str) -> Option<MeetingApplication> {
    let process = process.to_ascii_lowercase();
    let title = title.to_ascii_lowercase();
    let executable = process.trim_end_matches(".exe");
    if executable.contains("zoom") && contains_any(&title, &["zoom meeting", "meeting", "webinar"])
    {
        return Some(MeetingApplication::Zoom);
    }
    if matches!(executable, "ms-teams" | "teams") && contains_any(&title, &["meeting", "call"]) {
        return Some(MeetingApplication::Teams);
    }
    if matches!(executable, "chrome" | "msedge" | "firefox" | "brave")
        && contains_any(&title, &["meet -", "- meet", "google meet"])
    {
        return Some(MeetingApplication::GoogleMeet);
    }
    if executable == "slack" && contains_any(&title, &["huddle", "slack call"]) {
        return Some(MeetingApplication::Slack);
    }
    if executable == "discord" && contains_any(&title, &["voice", "call", "stream"]) {
        return Some(MeetingApplication::Discord);
    }
    None
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_supported_meeting_windows() {
        assert_eq!(
            classify_meeting("Zoom.exe", "Weekly sync - Zoom Meeting"),
            Some(MeetingApplication::Zoom)
        );
        assert_eq!(
            classify_meeting("chrome.exe", "Planning - Google Meet"),
            Some(MeetingApplication::GoogleMeet)
        );
        assert_eq!(classify_meeting("chrome.exe", "Inbox"), None);
        assert_eq!(classify_meeting("ms-teams.exe", "Microsoft Teams"), None);
        assert_eq!(classify_meeting("discord.exe", "Discord"), None);
        assert_eq!(
            classify_meeting("discord.exe", "Voice Connected - Project room"),
            Some(MeetingApplication::Discord)
        );
    }

    #[test]
    fn silence_offers_stop_after_sixty_seconds() {
        let activity = MeetingActivity {
            application: MeetingApplication::Teams,
            process_name: "ms-teams.exe".into(),
            window_title: "Call".into(),
        };
        let mut tracker = MeetingPromptTracker::default();
        tracker.set_recording(true);
        assert_eq!(
            tracker.observe(
                Some(activity.clone()),
                false,
                false,
                Duration::from_secs(59)
            ),
            None
        );
        assert_eq!(
            tracker.observe(Some(activity), false, false, Duration::from_secs(1)),
            Some(MeetingPromptEvent::OfferStop)
        );
    }

    #[test]
    fn closing_app_offers_stop_without_waiting() {
        let mut tracker = MeetingPromptTracker::default();
        tracker.set_recording(true);
        assert_eq!(
            tracker.observe(None, true, true, Duration::from_millis(250)),
            Some(MeetingPromptEvent::OfferStop)
        );
    }

    #[test]
    fn stop_prompt_is_emitted_once_per_recording_lifecycle() {
        let mut tracker = MeetingPromptTracker::default();
        tracker.set_recording(true);
        assert_eq!(
            tracker.observe(None, true, true, Duration::from_secs(2)),
            Some(MeetingPromptEvent::OfferStop)
        );
        assert_eq!(
            tracker.observe(None, false, false, Duration::from_secs(60)),
            None
        );

        tracker.set_recording(false);
        tracker.set_recording(true);
        assert_eq!(
            tracker.observe(None, true, true, Duration::from_secs(2)),
            Some(MeetingPromptEvent::OfferStop)
        );
    }

    #[test]
    fn dismissed_call_can_prompt_after_the_app_disappears() {
        let activity = MeetingActivity {
            application: MeetingApplication::Zoom,
            process_name: "zoom.exe".into(),
            window_title: "Zoom Meeting".into(),
        };
        let mut tracker = MeetingPromptTracker::default();
        assert!(matches!(
            tracker.observe(Some(activity.clone()), true, true, Duration::ZERO),
            Some(MeetingPromptEvent::OfferStart(_))
        ));
        tracker.dismiss_current(MeetingApplication::Zoom);
        assert_eq!(
            tracker.observe(Some(activity.clone()), true, true, Duration::ZERO),
            None
        );
        assert_eq!(tracker.observe(None, false, false, Duration::ZERO), None);
        assert!(matches!(
            tracker.observe(Some(activity), true, true, Duration::ZERO),
            Some(MeetingPromptEvent::OfferStart(_))
        ));
    }
}
