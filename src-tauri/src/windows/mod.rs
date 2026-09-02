//! Windows 11 integrations. Each service owns its native handles and exposes plain Rust reports
//! so persistence and UI policy remain in the application core.

pub mod audio;
pub mod context;
pub mod insertion;
pub mod meeting;
pub mod shell;
pub mod shortcut;

pub use audio::{
    list_microphones, CaptureActivity, CaptureEvent, MicrophoneDevice, WindowsAudioCapture,
};
pub use context::{observe_attributed_correction, ObservedCorrection};
pub use insertion::{InsertionTarget, WindowsTextInsertion};
