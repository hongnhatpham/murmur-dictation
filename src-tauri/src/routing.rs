use serde::{Deserialize, Serialize};

use crate::domain::{ProcessingMode, SessionKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SttProvider {
    DeepgramFlux,
    DeepgramNova3,
    AssemblyAi,
    GroqWhisper,
    LocalWhisper,
}

impl SttProvider {
    pub fn id(self) -> &'static str {
        match self {
            Self::DeepgramFlux => "deepgram_flux",
            Self::DeepgramNova3 => "deepgram_nova_3",
            Self::AssemblyAi => "assemblyai",
            Self::GroqWhisper => "groq_whisper_large_v3_turbo",
            Self::LocalWhisper => "local_whisper_large_v3_turbo_q5_0",
        }
    }

    pub fn is_local(self) -> bool {
        self == Self::LocalWhisper
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderFailure {
    QuotaExhausted,
    Authentication,
    Service,
    Timeout,
    PoorQuality,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteStatus {
    Active,
    Completed,
    Exhausted,
    Queued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteDecision {
    RetryCurrent,
    Switched,
    Completed,
    InsertRaw,
    QueueEnhancement,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAttempt {
    pub provider: SttProvider,
    pub failure: ProviderFailure,
    pub rotated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteState {
    pub session_kind: SessionKind,
    pub candidates: Vec<SttProvider>,
    pub current_index: usize,
    pub consecutive_timeouts: u8,
    pub status: RouteStatus,
    pub attempts: Vec<ProviderAttempt>,
}

impl RouteState {
    pub fn for_dictation(local_available: bool) -> Self {
        let mut candidates = vec![SttProvider::GroqWhisper];
        if local_available {
            candidates.push(SttProvider::LocalWhisper);
        }
        Self::new(SessionKind::Dictation, candidates)
    }

    pub fn for_meeting(local_available: bool) -> Self {
        let mut candidates = vec![SttProvider::GroqWhisper];
        if local_available {
            candidates.push(SttProvider::LocalWhisper);
        }
        Self::new(SessionKind::Meeting, candidates)
    }

    fn new(session_kind: SessionKind, candidates: Vec<SttProvider>) -> Self {
        Self {
            session_kind,
            candidates,
            current_index: 0,
            consecutive_timeouts: 0,
            status: RouteStatus::Active,
            attempts: vec![],
        }
    }

    pub fn current_provider(&self) -> Option<SttProvider> {
        (self.status == RouteStatus::Active)
            .then(|| self.candidates.get(self.current_index).copied())
            .flatten()
    }

    pub fn processing_mode(&self) -> ProcessingMode {
        match self.status {
            RouteStatus::Queued => ProcessingMode::Queued,
            _ => match self.current_provider() {
                Some(provider) if provider.is_local() => ProcessingMode::Local,
                Some(_) if self.current_index > 0 => ProcessingMode::HostedFallback,
                _ => ProcessingMode::Hosted,
            },
        }
    }

    pub fn succeed(&mut self) -> RouteDecision {
        self.status = RouteStatus::Completed;
        self.consecutive_timeouts = 0;
        RouteDecision::Completed
    }

    pub fn fail(&mut self, failure: ProviderFailure) -> RouteDecision {
        if self.status != RouteStatus::Active {
            return self.terminal_decision();
        }
        let provider = self.candidates[self.current_index];

        // Recognition quality is not an operational failure. Keep the same provider.
        if failure == ProviderFailure::PoorQuality {
            self.consecutive_timeouts = 0;
            self.attempts.push(ProviderAttempt {
                provider,
                failure,
                rotated: false,
            });
            return RouteDecision::RetryCurrent;
        }

        if failure == ProviderFailure::Timeout {
            self.consecutive_timeouts += 1;
            if self.consecutive_timeouts < 2 {
                self.attempts.push(ProviderAttempt {
                    provider,
                    failure,
                    rotated: false,
                });
                return RouteDecision::RetryCurrent;
            }
        }

        self.consecutive_timeouts = 0;
        let has_next = self.current_index + 1 < self.candidates.len();
        self.attempts.push(ProviderAttempt {
            provider,
            failure,
            rotated: has_next,
        });
        if has_next {
            self.current_index += 1;
            RouteDecision::Switched
        } else if self.session_kind == SessionKind::Meeting {
            self.status = RouteStatus::Queued;
            RouteDecision::QueueEnhancement
        } else {
            self.status = RouteStatus::Exhausted;
            RouteDecision::InsertRaw
        }
    }

    fn terminal_decision(&self) -> RouteDecision {
        match self.status {
            RouteStatus::Completed => RouteDecision::Completed,
            RouteStatus::Queued => RouteDecision::QueueEnhancement,
            RouteStatus::Exhausted => RouteDecision::InsertRaw,
            RouteStatus::Active => unreachable!(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CorrectionDecision {
    UseCorrected,
    InsertRaw,
}

/// Dictation correction never queues. A hosted failure immediately returns raw text.
pub fn correction_outcome(hosted_succeeded: bool) -> CorrectionDecision {
    if hosted_succeeded {
        CorrectionDecision::UseCorrected
    } else {
        CorrectionDecision::InsertRaw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poor_quality_does_not_rotate() {
        let mut route = RouteState::for_dictation(true);
        assert_eq!(
            route.fail(ProviderFailure::PoorQuality),
            RouteDecision::RetryCurrent
        );
        assert_eq!(route.current_provider(), Some(SttProvider::GroqWhisper));
    }

    #[test]
    fn transcript_routes_only_use_groq_and_optional_local() {
        assert_eq!(
            RouteState::for_dictation(false).candidates,
            vec![SttProvider::GroqWhisper]
        );
        assert_eq!(
            RouteState::for_meeting(true).candidates,
            vec![SttProvider::GroqWhisper, SttProvider::LocalWhisper]
        );
    }

    #[test]
    fn rotates_only_after_two_consecutive_timeouts() {
        let mut route = RouteState::for_dictation(true);
        assert_eq!(
            route.fail(ProviderFailure::Timeout),
            RouteDecision::RetryCurrent
        );
        assert_eq!(route.fail(ProviderFailure::Timeout), RouteDecision::Switched);
        assert_eq!(route.current_provider(), Some(SttProvider::LocalWhisper));
        assert_eq!(route.processing_mode(), ProcessingMode::Local);
    }

    #[test]
    fn dictation_exhaustion_inserts_raw() {
        let mut route = RouteState::for_dictation(false);
        assert_eq!(route.fail(ProviderFailure::QuotaExhausted), RouteDecision::InsertRaw);
        assert_eq!(route.status, RouteStatus::Exhausted);
    }

    #[test]
    fn meeting_exhaustion_queues() {
        let mut route = RouteState::for_meeting(false);
        route.fail(ProviderFailure::Unavailable);
        route.fail(ProviderFailure::Unavailable);
        assert_eq!(
            route.fail(ProviderFailure::Unavailable),
            RouteDecision::QueueEnhancement
        );
        assert_eq!(route.processing_mode(), ProcessingMode::Queued);
    }
}
