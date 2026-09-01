export type ProcessingMode = "hosted" | "hosted-fallback" | "local" | "queued";
export type SessionKind = "dictation" | "meeting";
export type InsertionStatus = "inserted" | "clipboard" | "raw" | "capturing" | "processing" | "completed" | "failed" | "cancelled" | "brief-ready" | "brief-pending";

export interface SessionSummary { id: string; kind: SessionKind; title: string; subtitle: string; time: string; mode: ProcessingMode; status: InsertionStatus; rawTranscript?: string; provider?: string }
export interface DictationDetail {
  id: string; result: string; rawTranscript: string; insertionStatus: "inserted" | "clipboard" | "failed"; processingMode: ProcessingMode;
  provider: string; targetApplication?: string; language: "English" | "Vietnamese" | "Mixed"; uncorrected: boolean;
  captureMs: number; transcriptionMs: number; correctionMs: number; insertionMs: number; expiresAt: string;
}
export interface TranscriptTurn { id: string; speaker: string; timestampSeconds: number; endSeconds?: number; text: string; rawText?: string; edited?: boolean }
export interface BriefItem { text: string; citationSeconds?: number }
export interface MeetingDetail {
  id: string; title: string; date: string; duration: string; durationSeconds?: number; speakers: number; expiresAt: string; pinned: boolean;
  audioPaths?: string[];
  status?: "recording" | "processing" | "pending" | "ready" | "failed"; pendingReason?: string; overview: string;
  topics?: BriefItem[]; decisions: BriefItem[]; actions: BriefItem[]; questions: BriefItem[]; notableMoments?: BriefItem[]; followUps?: BriefItem[]; turns: TranscriptTurn[];
}
export interface VocabularyEntry { id: string; spoken: string; replacement: string; source: "manual" | "learned"; uses: number }
export interface Preferences {
  contextCapture: boolean; launchAtStartup: boolean; meetingSuggestions: boolean; offlineModel: boolean;
  dictationRetentionDays: number; meetingRetentionDays: number; holdShortcut: string; toggleShortcut: string;
  microphoneId: string | null; excludedApplications: string[]; excludedMeetingApplications: string[];
}
export type CheckState = "ready" | "missing" | "checking" | "error" | "unavailable";
export interface SetupCheck { id: string; label: string; detail: string; state: CheckState }
export interface SetupStatus {
  checks: SetupCheck[];
  providers: Array<{ id: string; label: string; configured: boolean; state: CheckState; detail: string }>;
  update: { state: "current" | "checking" | "ready" | "error"; version?: string; detail: string };
}
export interface AudioDevice { id: string; label: string; isDefault: boolean }
export type DictationState = "idle" | "recording" | "transcribing" | "correcting" | "inserting" | "inserted" | "clipboard" | "raw" | "failed" | "cancelled";
export interface DictationEvent { sessionId?: string; state: DictationState; mode?: ProcessingMode; provider?: string; elapsedMs?: number; targetApp?: string; message?: string }
export interface DictationAudioLevelEvent { sessionId: string; rms: number; peak: number }
export interface MeetingEvent { sessionId?: string; state: "idle" | "recording" | "processing" | "pending" | "ready" | "failed"; elapsedMs?: number; message?: string }
export interface MeetingPromptEvent { kind: "start" | "stop"; appName: string; title?: string }
export interface AudioDeviceEvent { channel: "microphone" | "system"; state: "lost" | "restored"; message: string; sessionId?: string }
export interface SetupProgressEvent { operation: "offline-model" | "backup" | "restore" | "update"; percent: number; message: string }
export interface DataRemovalPreview { paths: string[]; bytes: number }

export const formatTimestamp = (seconds: number): string => `${Math.floor(seconds / 60)}:${Math.max(0, Math.floor(seconds % 60)).toString().padStart(2, "0")}`;
export const filterSessions = (sessions: SessionSummary[], query: string): SessionSummary[] => {
  const normalized = query.trim().toLocaleLowerCase();
  return normalized ? sessions.filter((session) => `${session.title} ${session.subtitle} ${session.rawTranscript ?? ""}`.toLocaleLowerCase().includes(normalized)) : sessions;
};
export const expiryLabel = (expiresAt: string, now = new Date()): string => {
  const days = Math.max(0, Math.ceil((new Date(expiresAt).getTime() - now.getTime()) / 86_400_000));
  return days === 0 ? "Expires today" : `${days} days left`;
};
