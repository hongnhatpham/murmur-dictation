use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

pub const MAX_AUDIO_CHUNK_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioChannel {
    Microphone,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingAudioInterval {
    pub channel: AudioChannel,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureStatus {
    Capturing,
    Stopped,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureState {
    pub status: CaptureStatus,
    pub microphone_active: bool,
    pub system_audio_active: bool,
    pub last_flushed_ms: u64,
    pub missing_intervals: Vec<MissingAudioInterval>,
}

impl CaptureState {
    pub fn dictation() -> Self {
        Self::new(false)
    }
    pub fn meeting() -> Self {
        Self::new(true)
    }

    fn new(system_audio_active: bool) -> Self {
        Self {
            status: CaptureStatus::Capturing,
            microphone_active: true,
            system_audio_active,
            last_flushed_ms: 0,
            missing_intervals: vec![],
        }
    }

    pub fn flush_chunk(&mut self, end_ms: u64) -> CoreResult<()> {
        self.ensure_capturing()?;
        if end_ms < self.last_flushed_ms || end_ms - self.last_flushed_ms > MAX_AUDIO_CHUNK_MS {
            return Err(CoreError::InvalidInput(
                "audio chunks must be ordered and no longer than five seconds".into(),
            ));
        }
        self.last_flushed_ms = end_ms;
        Ok(())
    }

    pub fn device_lost(&mut self, channel: AudioChannel, at_ms: u64) -> CoreResult<()> {
        self.ensure_capturing()?;
        let active = match channel {
            AudioChannel::Microphone => &mut self.microphone_active,
            AudioChannel::System => &mut self.system_audio_active,
        };
        if !*active {
            return Ok(());
        }
        *active = false;
        self.missing_intervals.push(MissingAudioInterval {
            channel,
            start_ms: at_ms,
            end_ms: None,
        });
        Ok(())
    }

    pub fn device_restored(&mut self, channel: AudioChannel, at_ms: u64) -> CoreResult<()> {
        self.ensure_capturing()?;
        let interval = self
            .missing_intervals
            .iter_mut()
            .rev()
            .find(|interval| interval.channel == channel && interval.end_ms.is_none())
            .ok_or_else(|| {
                CoreError::InvalidInput("device restoration has no matching loss interval".into())
            })?;
        if at_ms < interval.start_ms {
            return Err(CoreError::InvalidInput(
                "device restoration precedes device loss".into(),
            ));
        }
        interval.end_ms = Some(at_ms);
        match channel {
            AudioChannel::Microphone => self.microphone_active = true,
            AudioChannel::System => self.system_audio_active = true,
        }
        Ok(())
    }

    pub fn stop(&mut self, at_ms: u64) -> CoreResult<()> {
        self.ensure_capturing()?;
        for interval in self
            .missing_intervals
            .iter_mut()
            .filter(|interval| interval.end_ms.is_none())
        {
            interval.end_ms = Some(at_ms);
        }
        self.status = CaptureStatus::Stopped;
        Ok(())
    }

    /// Cancellation tells the native adapter to delete temporary audio immediately.
    pub fn cancel(&mut self) -> CoreResult<CaptureDisposition> {
        self.ensure_capturing()?;
        self.status = CaptureStatus::Cancelled;
        Ok(CaptureDisposition::DeleteTemporaryAudio)
    }

    fn ensure_capturing(&self) -> CoreResult<()> {
        if self.status == CaptureStatus::Capturing {
            Ok(())
        } else {
            Err(CoreError::InvalidInput(
                "capture is no longer active".into(),
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureDisposition {
    DeleteTemporaryAudio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InsertionStep {
    Accessibility,
    ClipboardPaste,
    RetainOnClipboard,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InsertionDecision {
    TryClipboardPaste,
    Finish,
    RetainClipboardAndFinish,
    RetainAndNotify,
    AlreadyFinished,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertionState {
    pub step: InsertionStep,
}

impl Default for InsertionState {
    fn default() -> Self {
        Self {
            step: InsertionStep::Accessibility,
        }
    }
}

impl InsertionState {
    pub fn report(&mut self, succeeded: bool) -> InsertionDecision {
        match (self.step, succeeded) {
            (InsertionStep::Accessibility, true) => {
                self.step = InsertionStep::Done;
                InsertionDecision::Finish
            }
            (InsertionStep::Accessibility, false) => {
                self.step = InsertionStep::ClipboardPaste;
                InsertionDecision::TryClipboardPaste
            }
            (InsertionStep::ClipboardPaste, true) => {
                self.step = InsertionStep::Done;
                InsertionDecision::RetainClipboardAndFinish
            }
            (InsertionStep::ClipboardPaste, false) => {
                self.step = InsertionStep::RetainOnClipboard;
                InsertionDecision::RetainAndNotify
            }
            (InsertionStep::RetainOnClipboard, _) | (InsertionStep::Done, _) => {
                InsertionDecision::AlreadyFinished
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPolicy {
    pub enabled: bool,
    pub denied_processes: HashSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusedTarget {
    pub process_name: String,
    pub is_password_field: bool,
    pub is_password_manager: bool,
    pub is_private_browser_window: bool,
    pub is_windows_security_dialog: bool,
}

impl ContextPolicy {
    pub fn may_capture(&self, target: &FocusedTarget) -> bool {
        self.enabled
            && !target.is_password_field
            && !target.is_password_manager
            && !target.is_private_browser_window
            && !target.is_windows_security_dialog
            && !self
                .denied_processes
                .contains(&target.process_name.to_ascii_lowercase())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeetingPromptDecision {
    OfferStart,
    OfferStop,
    StaySilent,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingPromptState {
    pub recording: bool,
    pub dismissed_for_current_call: bool,
    pub app_suppressed: bool,
}

impl MeetingPromptState {
    pub fn evaluate(
        &self,
        call_active: bool,
        both_sources_inactive_seconds: u64,
    ) -> MeetingPromptDecision {
        if self.app_suppressed || self.dismissed_for_current_call {
            return MeetingPromptDecision::StaySilent;
        }
        if !self.recording && call_active {
            MeetingPromptDecision::OfferStart
        } else if self.recording && (!call_active || both_sources_inactive_seconds >= 60) {
            MeetingPromptDecision::OfferStop
        } else {
            MeetingPromptDecision::StaySilent
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_records_device_gaps_without_switching() {
        let mut capture = CaptureState::meeting();
        capture.device_lost(AudioChannel::Microphone, 100).unwrap();
        assert!(!capture.microphone_active);
        assert!(capture.system_audio_active);
        capture
            .device_restored(AudioChannel::Microphone, 400)
            .unwrap();
        assert_eq!(capture.missing_intervals[0].end_ms, Some(400));
    }

    #[test]
    fn insertion_retains_text_after_both_methods_fail() {
        let mut insertion = InsertionState::default();
        assert_eq!(
            insertion.report(false),
            InsertionDecision::TryClipboardPaste
        );
        assert_eq!(insertion.report(false), InsertionDecision::RetainAndNotify);
    }

    #[test]
    fn clipboard_paste_success_retains_text() {
        let mut insertion = InsertionState::default();
        assert_eq!(
            insertion.report(false),
            InsertionDecision::TryClipboardPaste
        );
        assert_eq!(
            insertion.report(true),
            InsertionDecision::RetainClipboardAndFinish
        );
    }

    #[test]
    fn direct_insertion_does_not_request_clipboard_restore() {
        let mut insertion = InsertionState::default();
        assert_eq!(insertion.report(true), InsertionDecision::Finish);
    }

    #[test]
    fn sensitive_targets_are_hard_exclusions() {
        let policy = ContextPolicy {
            enabled: true,
            denied_processes: HashSet::new(),
        };
        let target = FocusedTarget {
            process_name: "browser".into(),
            is_password_field: false,
            is_password_manager: false,
            is_private_browser_window: true,
            is_windows_security_dialog: false,
        };
        assert!(!policy.may_capture(&target));
    }

    #[test]
    fn meeting_prompt_never_starts_or_stops_automatically() {
        let state = MeetingPromptState::default();
        assert_eq!(state.evaluate(true, 0), MeetingPromptDecision::OfferStart);
        let state = MeetingPromptState {
            recording: true,
            ..Default::default()
        };
        assert_eq!(state.evaluate(false, 0), MeetingPromptDecision::OfferStop);
    }
}
