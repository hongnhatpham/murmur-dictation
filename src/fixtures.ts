import type { MeetingDetail, Preferences, SessionSummary, VocabularyEntry } from "./domain";

export const sessions: SessionSummary[] = [
  { id: "import-04", kind: "meeting", title: "User interview 04", subtitle: "Imported recording · Waiting for connection", time: "14:30", mode: "queued", status: "brief-pending" },
  { id: "dictation-21", kind: "dictation", title: "Ping Nhật about the fixture set before end of day.", subtitle: "Slack · English + Vietnamese · 0:05", time: "13:41", mode: "hosted", status: "inserted", rawTranscript: "ping nhat about the fixture set before end of day", provider: "Deepgram Flux" },
  { id: "dictation-20", kind: "dictation", title: "Let's move the Thursday review to Friday morning.", subtitle: "Outlook · Hosted fallback · 0:09", time: "11:52", mode: "hosted-fallback", status: "clipboard", rawTranscript: "lets move the thursday review to friday friday morning", provider: "AssemblyAI fallback" },
  { id: "dictation-19", kind: "dictation", title: "Không sao, mình gửi lại bản tóm tắt sau cuộc họp.", subtitle: "Slack · Offline · 0:08", time: "11:18", mode: "local", status: "raw", rawTranscript: "Không sao mình gửi lại bản tóm tắt sau cuộc họp", provider: "Local Whisper" },
  { id: "product-sync", kind: "meeting", title: "Product sync", subtitle: "42:16 · 4 speakers · 23 days left", time: "10:00", mode: "hosted", status: "brief-ready", provider: "Deepgram Nova-3" },
];

export const meeting: MeetingDetail = {
  id: "product-sync", title: "Product sync", date: "31 August", duration: "42:16", speakers: 4,
  expiresAt: "2026-09-30T00:00:00", pinned: false,
  overview: "The team settled V1 capture order, confirmed that meeting transcription runs after recording, and assigned the bilingual fixture set.",
  decisions: [
    { text: "Imported-file processing ships before live capture.", citationSeconds: 724 },
    { text: "Meeting transcription runs after recording stops in V1.", citationSeconds: 782 },
  ],
  actions: [
    { text: "Mai will build the English, Vietnamese, and mixed-language fixture set.", citationSeconds: 731 },
    { text: "Sam will close the two open WASAPI capture bugs before Friday.", citationSeconds: 718 },
  ],
  questions: [{ text: "Does in-person capture need a second microphone?", citationSeconds: 1747 }],
  turns: [
    { id: "t1", speaker: "Sam", timestampSeconds: 718, text: "We still have two capture bugs open on the WASAPI side." },
    { id: "t2", speaker: "You", timestampSeconds: 724, text: "Let's ship imported-file processing first. It lets us judge transcript quality without confusing it with capture bugs." },
    { id: "t3", speaker: "Sam", timestampSeconds: 729, text: "Agreed. That isolates the variable." },
    { id: "t4", speaker: "Mai", timestampSeconds: 731, text: "Được, mình sẽ lo bộ dữ liệu mẫu, cả tiếng Anh, tiếng Việt và câu trộn hai thứ tiếng.", edited: true },
    { id: "t5", speaker: "You", timestampSeconds: 782, text: "No live meeting transcript in V1. We process after the recording stops." },
  ],
};

export const vocabulary: VocabularyEntry[] = [
  { id: "v1", spoken: "not", replacement: "Nhật", source: "learned", uses: 7 },
  { id: "v2", spoken: "was api", replacement: "WASAPI", source: "manual", uses: 5 },
  { id: "v3", spoken: "murmur", replacement: "Murmur", source: "manual", uses: 18 },
  { id: "v4", spoken: "tóm tắc", replacement: "tóm tắt", source: "learned", uses: 4 },
];

export const defaultPreferences: Preferences = {
  contextCapture: true, launchAtStartup: true, meetingSuggestions: true, offlineModel: true,
  dictationRetentionDays: 7, meetingRetentionDays: 30,
  holdShortcut: "Ctrl+Win", toggleShortcut: "Ctrl+Win+Space", microphoneId: null,
  excludedApplications: ["1Password", "Bitwarden", "KeePassXC", "Windows Security"],
  excludedMeetingApplications: [],
};
