import type { AudioDevice, AudioDeviceEvent, DataRemovalPreview, DictationAudioLevelEvent, DictationDetail, DictationEvent, MeetingDetail, MeetingEvent, MeetingPromptEvent, Preferences, SessionSummary, SetupProgressEvent, SetupStatus, VocabularyEntry } from "./domain";

type NativeSession = { id: string; kind: "dictation" | "meeting"; title: string | null; status: "capturing" | "processing" | "completed" | "cancelled" | "failed" | "pendingEnhancement"; processingMode: "hosted" | "hostedFallback" | "local" | "queued" | null; provider: string | null; createdAt: string };
export type NativeBrief = { overview: string; items: Array<{ kind: "decision" | "actionItem" | "openQuestion" | "notableMoment" | "possibleFollowUp"; text: string; citations: Array<{ startMs: number }> }> };
export interface NativeEventMap {
  "murmur://dictation-state": DictationEvent;
  "murmur://dictation-audio-level": DictationAudioLevelEvent;
  "murmur://meeting-state": MeetingEvent;
  "murmur://meeting-prompt": MeetingPromptEvent;
  "murmur://connectivity": { online: boolean };
  "murmur://sessions-changed": Record<string, never>;
  "murmur://audio-device": AudioDeviceEvent;
  "murmur://setup-progress": SetupProgressEvent;
}
export interface MurmurBridge {
  readonly isNative: boolean;
  listen<K extends keyof NativeEventMap>(event: K, handler: (payload: NativeEventMap[K]) => void): Promise<() => void>;
  copyText(text: string): Promise<void>; deleteSession(id: string): Promise<void>; listSessions(query?: string): Promise<SessionSummary[]>;
  getDictationDetail(id: string): Promise<DictationDetail>; transformDictation(id: string, action: "rewrite" | "translate-english" | "translate-vietnamese"): Promise<DictationDetail>;
  getMeetingDetail(id: string): Promise<MeetingDetail>; getMeetingBrief(id: string): Promise<NativeBrief | null>;
  startDictation(): Promise<void>; stopDictation(): Promise<void>; cancelDictation(): Promise<void>;
  importRecording(): Promise<string | null>; startMeeting(source: "online" | "in-person"): Promise<string>; stopMeeting(): Promise<void>; resumeEnhancement(id: string): Promise<void>;
  dismissMeetingPrompt(appName: string, disableForApp: boolean): Promise<void>;
  saveTranscriptTurn(sessionId: string, turnId: string, text: string): Promise<void>; renameSpeaker(sessionId: string, from: string, to: string): Promise<void>; seekMeetingAudio(sessionId: string, seconds: number): Promise<void>;
  setMeetingPinned(id: string, pinned: boolean): Promise<void>; exportMeeting(id: string): Promise<string | null>;
  listVocabulary(): Promise<VocabularyEntry[]>; addVocabulary(spoken: string, replacement: string): Promise<VocabularyEntry>; updateVocabulary(entry: VocabularyEntry): Promise<void>; deleteVocabulary(id: string): Promise<void>; resetLearnedVocabulary(): Promise<void>;
  getPreferences(): Promise<Preferences>; savePreferences(preferences: Preferences): Promise<void>; getSetupStatus(): Promise<SetupStatus>; validateProvider(providerId: string): Promise<SetupStatus>; listAudioDevices(): Promise<AudioDevice[]>; chooseContextApplication(): Promise<string | null>;
  setProviderCredential(providerId: string, secret: string): Promise<SetupStatus>;
  runSetupCheck(checkId: string): Promise<SetupStatus>; installOfflineModel(): Promise<SetupStatus>;
  createBackup(): Promise<string | null>; restoreBackup(): Promise<string | null>; exportDiagnostics(): Promise<string | null>; checkForUpdates(): Promise<SetupStatus>;
  previewDataRemoval(): Promise<DataRemovalPreview>; removeUserData(confirmation: "DELETE"): Promise<void>;
}

const runningInTauri = (): boolean => "__TAURI_INTERNALS__" in window;
const invokeNative = async <T>(command: string, args?: Record<string, unknown>): Promise<T> => { const { invoke } = await import("@tauri-apps/api/core"); return invoke<T>(command, args); };
const nativeOnly = () => { if (!runningInTauri()) throw new Error("This action is available in the Windows app only."); };
const toSummary = (session: NativeSession): SessionSummary => {
  const created = new Date(session.createdAt);
  const mode = session.processingMode === "hostedFallback" ? "hosted-fallback" : (session.processingMode ?? "queued");
  const statuses = { capturing: "capturing", processing: "processing", completed: "completed", cancelled: "cancelled", failed: "failed", pendingEnhancement: "brief-pending" } as const;
  return { id: session.id, kind: session.kind, title: session.title ?? (session.kind === "meeting" ? "Untitled meeting" : "Untitled dictation"), subtitle: [session.provider, session.status.replace(/([A-Z])/g, " $1").toLowerCase()].filter(Boolean).join(" · "), time: Number.isNaN(created.getTime()) ? "" : created.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }), mode, status: session.kind === "meeting" && session.status === "completed" ? "brief-ready" : statuses[session.status], provider: session.provider ?? undefined };
};

export const bridge: MurmurBridge = {
  get isNative() { return runningInTauri(); },
  async listen(event, handler) { if (!runningInTauri() || typeof (window as unknown as { __TAURI_INTERNALS__?: { transformCallback?: unknown } }).__TAURI_INTERNALS__?.transformCallback !== "function") return () => undefined; const { listen } = await import("@tauri-apps/api/event"); return listen(event, ({ payload }) => handler(payload as NativeEventMap[typeof event])); },
  async copyText(text) { if (!navigator.clipboard) throw new Error("Clipboard access is unavailable."); await navigator.clipboard.writeText(text); },
  async deleteSession(id) { nativeOnly(); await invokeNative("delete_session", { sessionId: id }); },
  async getDictationDetail(id) { nativeOnly(); return invokeNative("get_dictation_detail", { sessionId: id }); }, async transformDictation(id, action) { nativeOnly(); return invokeNative("transform_dictation", { sessionId: id, action }); },
  async listSessions(query = "") { nativeOnly(); if (query.trim()) { const ids = await invokeNative<string[]>("search_sessions", { query, limit: 50 }); const allowed = new Set(ids); return (await invokeNative<NativeSession[]>("list_sessions", { kind: null })).filter((s) => allowed.has(s.id)).map(toSummary); } return (await invokeNative<NativeSession[]>("list_sessions", { kind: null })).map(toSummary); },
  async getMeetingDetail(id) { nativeOnly(); return invokeNative("get_meeting_detail", { sessionId: id }); }, async getMeetingBrief(id) { nativeOnly(); return invokeNative("get_meeting_brief", { sessionId: id }); },
  async startDictation() { nativeOnly(); await invokeNative("start_dictation"); }, async stopDictation() { nativeOnly(); await invokeNative("stop_dictation"); }, async cancelDictation() { nativeOnly(); await invokeNative("cancel_dictation"); },
  async importRecording() { nativeOnly(); return invokeNative("import_recording"); }, async startMeeting(source) { nativeOnly(); return invokeNative("start_meeting", { source }); }, async stopMeeting() { nativeOnly(); await invokeNative("stop_meeting"); }, async resumeEnhancement(id) { nativeOnly(); await invokeNative("resume_enhancement", { sessionId: id }); },
  async dismissMeetingPrompt(appName, disableForApp) { nativeOnly(); await invokeNative("dismiss_meeting_prompt", { appName, disableForApp }); },
  async saveTranscriptTurn(sessionId, turnId, text) { nativeOnly(); await invokeNative("save_transcript_turn", { sessionId, turnId, text }); }, async renameSpeaker(sessionId, from, to) { nativeOnly(); await invokeNative("rename_speaker", { sessionId, from, to }); }, async seekMeetingAudio(sessionId, seconds) { nativeOnly(); await invokeNative("seek_meeting_audio", { sessionId, seconds }); },
  async setMeetingPinned(id, pinned) { nativeOnly(); await invokeNative("set_meeting_pinned", { sessionId: id, pinned }); }, async exportMeeting(id) { nativeOnly(); return invokeNative("export_meeting", { sessionId: id }); },
  async listVocabulary() { nativeOnly(); return invokeNative("list_vocabulary"); }, async addVocabulary(spoken, replacement) { nativeOnly(); return invokeNative("add_vocabulary", { spoken, replacement }); }, async updateVocabulary(entry) { nativeOnly(); await invokeNative("update_vocabulary", { entry }); }, async deleteVocabulary(id) { nativeOnly(); await invokeNative("delete_vocabulary", { id }); }, async resetLearnedVocabulary() { nativeOnly(); await invokeNative("reset_learned_vocabulary"); },
  async getPreferences() { nativeOnly(); return invokeNative("get_preferences"); }, async savePreferences(preferences) { nativeOnly(); await invokeNative("save_preferences", { preferences }); }, async getSetupStatus() { nativeOnly(); return invokeNative("get_setup_status"); }, async validateProvider(providerId) { nativeOnly(); return invokeNative("validate_provider", { providerId }); }, async listAudioDevices() { nativeOnly(); return invokeNative("list_audio_devices"); }, async chooseContextApplication() { nativeOnly(); return invokeNative("choose_context_application"); },
  async setProviderCredential(providerId, secret) { nativeOnly(); return invokeNative("set_provider_credential", { providerId, secret }); },
  async runSetupCheck(checkId) { nativeOnly(); return invokeNative("run_setup_check", { checkId }); }, async installOfflineModel() { nativeOnly(); return invokeNative("install_offline_model"); },
  async createBackup() { nativeOnly(); return invokeNative("create_backup"); }, async restoreBackup() { nativeOnly(); return invokeNative("restore_backup"); }, async exportDiagnostics() { nativeOnly(); return invokeNative("export_diagnostics"); }, async checkForUpdates() { nativeOnly(); return invokeNative("check_for_updates"); },
  async previewDataRemoval() { nativeOnly(); return invokeNative("preview_data_removal"); }, async removeUserData(confirmation) { nativeOnly(); await invokeNative("remove_user_data", { confirmation }); },
};
