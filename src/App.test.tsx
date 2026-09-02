// @vitest-environment jsdom
import "@testing-library/jest-dom/vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { bridge } from "./bridge";
import { defaultPreferences } from "./fixtures";
import type { MeetingDetail, SetupStatus } from "./domain";

const nativeSetup: SetupStatus = { checks: [], providers: [], update: { state: "current", detail: "Murmur is current" } };
const nativeMeeting: MeetingDetail = { id: "meeting-1", title: "Native meeting", date: "August 31, 2026", duration: "1:00", durationSeconds: 60, speakers: 1, expiresAt: "2026-09-30T00:00:00Z", pinned: false, status: "ready", overview: "A saved brief.", decisions: [], actions: [], questions: [], turns: [{ id: "turn-1", speaker: "You", timestampSeconds: 2, text: "Original transcript", rawText: "Original transcript" }] };

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  window.history.pushState({}, "", "/");
});
beforeEach(() => localStorage.setItem("murmur.onboarding.complete", "1"));

describe("Murmur app", () => {
  it("opens the meeting record from navigation", () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Meetings" }));
    expect(screen.getByRole("heading", { name: "Product sync" })).toBeInTheDocument();
    expect(screen.getByText("Imported-file processing ships before live capture.")).toBeInTheDocument();
  });

  it("copies every Meeting Brief section with citations", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([{ id: "meeting-1", kind: "meeting", title: "Native meeting", subtitle: "ready", time: "10:00", mode: "hosted", status: "brief-ready" }]);
    vi.spyOn(bridge, "getMeetingDetail").mockResolvedValue({
      ...nativeMeeting,
      topics: [{ text: "Release scope", citationSeconds: 4 }],
      decisions: [{ text: "Ship it.", citationSeconds: 8 }],
      notableMoments: [{ text: "The demo worked.", citationSeconds: 12 }],
      followUps: [{ text: "Check adoption.", citationSeconds: 16 }],
    });
    const copy = vi.spyOn(bridge, "copyText").mockResolvedValue();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Meetings" }));
    fireEvent.click(await screen.findByRole("button", { name: "Copy brief" }));
    await waitFor(() => expect(copy).toHaveBeenCalledOnce());
    const copied = copy.mock.calls[0][0];
    expect(copied).toContain("Detailed topics");
    expect(copied).toContain("Notable moments");
    expect(copied).toContain("Possible follow-ups");
    expect(copied).toMatch(/Decisions[\s\S]*\[\d+:\d{2}\]/);
  });

  it("does not claim an incomplete Meeting Brief is cited", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([{ id: "meeting-1", kind: "meeting", title: "Native meeting", subtitle: "ready", time: "10:00", mode: "hosted", status: "brief-ready" }]);
    vi.spyOn(bridge, "getMeetingDetail").mockResolvedValue({ ...nativeMeeting, decisions: [{ text: "Ship it without a citation." }] });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Meetings" }));
    expect(await screen.findByText("Citations incomplete")).toBeInTheDocument();
    expect(screen.queryByText("Cited")).not.toBeInTheDocument();
  });

  it("filters session history", () => {
    render(<App />);
    fireEvent.change(screen.getByRole("textbox", { name: "Search sessions" }), { target: { value: "Không sao" } });
    expect(screen.getByText("Không sao, mình gửi lại bản tóm tắt sau cuộc họp.")).toBeInTheDocument();
    expect(screen.queryByText("User interview 04")).not.toBeInTheDocument();
  });

  it("lets the user disable context capture", () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    const contextToggle = screen.getByRole("switch", { name: "Capture focused context" });
    expect(contextToggle).toHaveAttribute("aria-checked", "true");
    fireEvent.click(contextToggle);
    expect(contextToggle).toHaveAttribute("aria-checked", "false");
  });

  it("labels browser fixtures as sample data", () => {
    render(<App />);
    expect(screen.getByText("Sample data · Browser preview")).toBeInTheDocument();
  });

  it("loads native sessions without showing browser fixtures", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValueOnce([{
      id: "native-1",
      kind: "dictation",
      title: "A local dictation",
      subtitle: "completed",
      time: "09:12",
      mode: "hosted",
      status: "inserted",
    }]);
    render(<App />);
    expect(await screen.findByText("A local dictation")).toBeInTheDocument();
    expect(screen.queryByText("Sample data · Browser preview")).not.toBeInTheDocument();
    expect(screen.queryByText("User interview 04")).not.toBeInTheDocument();
  });

  it("opens a failed dictation without requesting a result that was never created", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValueOnce([{
      id: "failed-dictation",
      kind: "dictation",
      title: "Dictation",
      subtitle: "failed",
      time: "11:32",
      mode: "queued",
      status: "failed",
    }]);
    const getDetail = vi.spyOn(bridge, "getDictationDetail").mockRejectedValue(
      new Error("not found: dictation result for failed-dictation"),
    );
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: /Dictation/ }));
    expect(getDetail).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByText("Processing route unavailable · Failed")).toBeInTheDocument();
  });

  it("does not show Listening when native capture fails", async () => {
    vi.spyOn(bridge, "startDictation").mockRejectedValueOnce(new Error("capture adapter is unavailable"));
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Start dictation" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Recording did not start");
    expect(screen.queryByText("Listening")).not.toBeInTheDocument();
  });

  it("shows onboarding until explicit consent and completion", () => {
    localStorage.removeItem("murmur.onboarding.complete");
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    const continueButton = screen.getByRole("button", { name: "Continue" });
    expect(continueButton).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox"));
    expect(continueButton).toBeEnabled();
  });

  it("shows native dictation lifecycle events and exposes stop and cancel", async () => {
    window.history.pushState({}, "", "/?overlay=1");
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([]);
    const handlers = new Map<string, (payload: unknown) => void>();
    const unlisteners = new Map<string, ReturnType<typeof vi.fn>>();
    vi.spyOn(bridge, "listen").mockImplementation(async (event, handler) => {
      handlers.set(event, handler as (payload: unknown) => void);
      const off = vi.fn();
      unlisteners.set(event, off);
      return off;
    });
    const stop = vi.spyOn(bridge, "stopDictation").mockResolvedValue();
    const cancel = vi.spyOn(bridge, "cancelDictation").mockResolvedValue();
    render(<App />);
    await waitFor(() => expect(handlers.has("murmur://dictation-state")).toBe(true));
    act(() => handlers.get("murmur://dictation-state")?.({ sessionId: "dictation-1", state: "recording", elapsedMs: 3200 }));
    expect(screen.getByRole("status")).toHaveTextContent(/^Listening$/);
    const signal = document.querySelector<HTMLElement>("[data-speech-signal]");
    expect(signal).toHaveAttribute("aria-hidden", "true");
    await waitFor(() => expect(handlers.has("murmur://dictation-audio-level")).toBe(true));
    const lastBar = signal?.lastElementChild as HTMLElement;
    expect(lastBar.style.transform).toBe("scaleY(0.08)");
    act(() => handlers.get("murmur://dictation-audio-level")?.({ sessionId: "another-session", rms: 0.3, peak: 0.5 }));
    expect(lastBar.style.transform).toBe("scaleY(0.08)");
    act(() => handlers.get("murmur://dictation-audio-level")?.({ sessionId: "dictation-1", rms: 0.3, peak: 0.5 }));
    expect(lastBar.style.transform).not.toBe("scaleY(0.08)");
    act(() => handlers.get("murmur://dictation-state")?.({ sessionId: "dictation-1", state: "recording", elapsedMs: 4200 }));
    expect(screen.getByRole("status")).toHaveTextContent(/^Listening$/);
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel dictation" }));
    expect(stop).toHaveBeenCalledOnce();
    expect(cancel).toHaveBeenCalledOnce();
    act(() => handlers.get("murmur://dictation-state")?.({ state: "raw", mode: "local", provider: "Local Whisper" }));
    expect(screen.getByRole("status")).toHaveTextContent(/^Inserted raw transcript$/);
    expect(screen.getByText("Local Whisper")).toBeInTheDocument();
    await waitFor(() => expect(unlisteners.get("murmur://dictation-audio-level")).toHaveBeenCalledOnce());
  });

  it("offers explicit meeting start from a native meeting prompt", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([]);
    const handlers = new Map<string, (payload: unknown) => void>();
    vi.spyOn(bridge, "listen").mockImplementation(async (event, handler) => {
      handlers.set(event, handler as (payload: unknown) => void);
      return () => undefined;
    });
    const start = vi.spyOn(bridge, "startMeeting").mockResolvedValue("meeting-1");
    render(<App />);
    await waitFor(() => expect(handlers.has("murmur://meeting-prompt")).toBe(true));
    act(() => handlers.get("murmur://meeting-prompt")?.({ kind: "start", appName: "Microsoft Teams", title: "Weekly sync" }));
    fireEvent.click(screen.getByRole("button", { name: "Start recording" }));
    await waitFor(() => expect(start).toHaveBeenCalledWith("online"));
    expect(screen.queryByRole("dialog", { name: "Meeting suggestion" })).not.toBeInTheDocument();
  });

  it("renders only dictation feedback in the dedicated overlay window", async () => {
    window.history.pushState({}, "", "/?overlay=1");
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    const handlers = new Map<string, (payload: unknown) => void>();
    vi.spyOn(bridge, "listen").mockImplementation(async (event, handler) => { handlers.set(event, handler as (payload: unknown) => void); return () => undefined; });
    render(<App />);
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
    expect(screen.getByRole("alert")).toBeEmptyDOMElement();
    await waitFor(() => expect(handlers.has("murmur://dictation-state")).toBe(true));
    act(() => handlers.get("murmur://dictation-state")?.({ state: "transcribing", mode: "hosted", provider: "Deepgram Flux" }));
    expect(screen.getByRole("status")).toHaveTextContent(/^Transcribing$/);
    expect(document.querySelector(".dictation-overlay")).toHaveAttribute("data-state", "transcribing");
    expect(screen.queryByRole("banner")).not.toBeInTheDocument();
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
    window.history.pushState({}, "", "/");
  });

  it("does not accept audio levels without a scoped dictation session", async () => {
    window.history.pushState({}, "", "/?overlay=1");
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    const handlers = new Map<string, (payload: unknown) => void>();
    vi.spyOn(bridge, "listen").mockImplementation(async (event, handler) => { handlers.set(event, handler as (payload: unknown) => void); return () => undefined; });
    render(<App />);
    await waitFor(() => expect(handlers.has("murmur://dictation-state")).toBe(true));
    act(() => handlers.get("murmur://dictation-state")?.({ state: "recording" }));
    await act(async () => { await Promise.resolve(); });
    expect(handlers.has("murmur://dictation-audio-level")).toBe(false);
    window.history.pushState({}, "", "/");
  });

  it("keeps the recording signal static when reduced motion is requested", async () => {
    window.history.pushState({}, "", "/?overlay=1");
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    const originalMatchMedia = window.matchMedia;
    Object.defineProperty(window, "matchMedia", { configurable: true, value: vi.fn(() => ({ matches: true })) });
    const handlers = new Map<string, (payload: unknown) => void>();
    vi.spyOn(bridge, "listen").mockImplementation(async (event, handler) => { handlers.set(event, handler as (payload: unknown) => void); return () => undefined; });
    render(<App />);
    await waitFor(() => expect(handlers.has("murmur://dictation-state")).toBe(true));
    act(() => handlers.get("murmur://dictation-state")?.({ sessionId: "dictation-1", state: "recording" }));
    await act(async () => { await Promise.resolve(); });
    expect(handlers.has("murmur://dictation-audio-level")).toBe(false);
    Object.defineProperty(window, "matchMedia", { configurable: true, value: originalMatchMedia });
    window.history.pushState({}, "", "/");
  });

  it("gives every dictation state distinct status semantics", async () => {
    window.history.pushState({}, "", "/?overlay=1");
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    const handlers = new Map<string, (payload: unknown) => void>();
    vi.spyOn(bridge, "listen").mockImplementation(async (event, handler) => { handlers.set(event, handler as (payload: unknown) => void); return () => undefined; });
    render(<App />);
    await waitFor(() => expect(handlers.has("murmur://dictation-state")).toBe(true));
    const cases = [
      [{ state: "recording" }, "recording", "Listening", "status"],
      [{ state: "transcribing" }, "transcribing", "Transcribing", "status"],
      [{ state: "correcting" }, "correcting", "Correcting", "status"],
      [{ state: "inserting" }, "inserting", "Inserting", "status"],
      [{ state: "inserted", targetApp: "Outlook" }, "success", "Inserted into Outlook", "status"],
      [{ state: "clipboard" }, "fallback", "Kept on clipboard", "status"],
      [{ state: "raw" }, "fallback", "Inserted raw transcript", "status"],
      [{ state: "cancelled" }, "cancelled", "Cancelled", "status"],
      [{ state: "failed" }, "failed", "Dictation failed", "alert"],
      [{ state: "transcribing", mode: "hosted-fallback" }, "fallback", "Transcribing", "status"],
    ] as const;
    for (const [payload, visualState, label, role] of cases) {
      act(() => handlers.get("murmur://dictation-state")?.(payload));
      expect(document.querySelector(".dictation-overlay")).toHaveAttribute("data-state", visualState);
      expect(screen.getByRole(role)).toHaveTextContent(new RegExp(`^${label}$`));
    }
    window.history.pushState({}, "", "/");
  });

  it("can suppress meeting suggestions for a detected app", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([]);
    const handlers = new Map<string, (payload: unknown) => void>();
    vi.spyOn(bridge, "listen").mockImplementation(async (event, handler) => { handlers.set(event, handler as (payload: unknown) => void); return () => undefined; });
    const dismiss = vi.spyOn(bridge, "dismissMeetingPrompt").mockResolvedValue();
    render(<App />);
    await waitFor(() => expect(handlers.has("murmur://meeting-prompt")).toBe(true));
    act(() => handlers.get("murmur://meeting-prompt")?.({ kind: "start", appName: "Discord" }));
    fireEvent.click(screen.getByRole("button", { name: "Don't suggest for Discord" }));
    await waitFor(() => expect(dismiss).toHaveBeenCalledWith("Discord", true));
  });

  it("does not offer app suppression on a stop prompt", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([]);
    const handlers = new Map<string, (payload: unknown) => void>();
    vi.spyOn(bridge, "listen").mockImplementation(async (event, handler) => { handlers.set(event, handler as (payload: unknown) => void); return () => undefined; });
    render(<App />);
    await waitFor(() => expect(handlers.has("murmur://meeting-prompt")).toBe(true));
    act(() => handlers.get("murmur://meeting-prompt")?.({ kind: "stop", appName: "Detected meeting" }));
    expect(screen.getByRole("button", { name: "Stop recording" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Don't suggest/ })).not.toBeInTheDocument();
  });

  it("does not duplicate dictation feedback in the main window", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([]);
    const handlers = new Map<string, (payload: unknown) => void>();
    vi.spyOn(bridge, "listen").mockImplementation(async (event, handler) => { handlers.set(event, handler as (payload: unknown) => void); return () => undefined; });
    render(<App />);
    await waitFor(() => expect(handlers.has("murmur://dictation-state")).toBe(true));
    act(() => handlers.get("murmur://dictation-state")?.({ state: "recording", elapsedMs: 500 }));
    expect(screen.queryByText("Listening")).not.toBeInTheDocument();
  });

  it("does not apply a transcript edit after native save rejects it", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([{ id: "meeting-1", kind: "meeting", title: "Native meeting", subtitle: "ready", time: "10:00", mode: "hosted", status: "brief-ready" }]);
    vi.spyOn(bridge, "getMeetingDetail").mockResolvedValue(nativeMeeting);
    vi.spyOn(bridge, "saveTranscriptTurn").mockRejectedValue(new Error("database write failed"));
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Meetings" }));
    expect(await screen.findByRole("heading", { name: "Native meeting" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Original transcript" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Edit transcript at 0:02" }), { target: { value: "Unsaved edit" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("database write failed");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("button", { name: "Original transcript" })).toBeInTheDocument();
    expect(screen.queryByText("Unsaved edit")).not.toBeInTheDocument();
  });

  it("refreshes an open meeting when local transcription finishes", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([{ id: "meeting-1", kind: "meeting", title: "Native meeting", subtitle: "processing", time: "10:00", mode: "local", status: "processing" }]);
    const pending = { ...nativeMeeting, status: "processing" as const, turns: [] };
    vi.spyOn(bridge, "getMeetingDetail").mockResolvedValueOnce(pending).mockResolvedValue(nativeMeeting);
    const handlers = new Map<string, (payload: unknown) => void>();
    vi.spyOn(bridge, "listen").mockImplementation(async (event, handler) => { handlers.set(event, handler as (payload: unknown) => void); return () => undefined; });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Meetings" }));
    expect(await screen.findByText("Transcript not ready")).toBeInTheDocument();
    await waitFor(() => expect(handlers.has("murmur://sessions-changed")).toBe(true));

    act(() => handlers.get("murmur://sessions-changed")?.({}));

    expect(await screen.findByRole("button", { name: "Original transcript" })).toBeInTheDocument();
  });

  it("preserves suppressed meeting applications when saving preferences", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([]);
    vi.spyOn(bridge, "getPreferences").mockResolvedValue({ ...defaultPreferences, excludedMeetingApplications: ["Discord"] });
    vi.spyOn(bridge, "getSetupStatus").mockResolvedValue(nativeSetup);
    vi.spyOn(bridge, "listAudioDevices").mockResolvedValue([]);
    const save = vi.spyOn(bridge, "savePreferences").mockResolvedValue();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await waitFor(() => expect(screen.getByDisplayValue("Ctrl+Win")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(save).toHaveBeenCalledWith(expect.objectContaining({ excludedMeetingApplications: ["Discord"] })));
  });

  it("treats a canceled backup picker as neutral", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([]);
    vi.spyOn(bridge, "getPreferences").mockResolvedValue(defaultPreferences);
    vi.spyOn(bridge, "getSetupStatus").mockResolvedValue(nativeSetup);
    vi.spyOn(bridge, "listAudioDevices").mockResolvedValue([]);
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const backup = vi.spyOn(bridge, "createBackup").mockResolvedValue(null);
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(await screen.findByRole("button", { name: "Create backup" }));
    await waitFor(() => expect(backup).toHaveBeenCalledOnce());
    expect(screen.queryByText("Backup saved.")).not.toBeInTheDocument();
  });

  it("surfaces a resolved update error as an error", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([]);
    vi.spyOn(bridge, "getPreferences").mockResolvedValue(defaultPreferences);
    vi.spyOn(bridge, "getSetupStatus").mockResolvedValue(nativeSetup);
    vi.spyOn(bridge, "listAudioDevices").mockResolvedValue([]);
    vi.spyOn(bridge, "checkForUpdates").mockResolvedValue({ ...nativeSetup, update: { state: "error", detail: "Private update access is not configured" } });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(await screen.findByRole("button", { name: "Check now" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Private update access is not configured");
  });

  it("loads preferences when setup status and microphone listing fail", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([]);
    vi.spyOn(bridge, "getPreferences").mockResolvedValue({ ...defaultPreferences, holdShortcut: "Ctrl+Shift+M" });
    vi.spyOn(bridge, "getSetupStatus").mockRejectedValue(new Error("setup unavailable"));
    vi.spyOn(bridge, "listAudioDevices").mockRejectedValue(new Error("audio unavailable"));
    const save = vi.spyOn(bridge, "savePreferences").mockResolvedValue();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(await screen.findByDisplayValue("Ctrl+Shift+M")).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(save).toHaveBeenCalledWith(expect.objectContaining({ holdShortcut: "Ctrl+Shift+M" })));
  });

  it("retries failed Settings loads in place", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([]);
    const preferences = vi.spyOn(bridge, "getPreferences").mockRejectedValueOnce(new Error("preferences offline")).mockResolvedValue(defaultPreferences);
    const setup = vi.spyOn(bridge, "getSetupStatus").mockRejectedValueOnce(new Error("setup offline")).mockResolvedValue(nativeSetup);
    const microphones = vi.spyOn(bridge, "listAudioDevices").mockRejectedValueOnce(new Error("audio offline")).mockResolvedValue([]);
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    fireEvent.click(await screen.findByRole("button", { name: "Retry preferences" }));
    fireEvent.click(screen.getByRole("button", { name: "Retry setup" }));
    fireEvent.click(screen.getByRole("button", { name: "Retry microphones" }));
    await waitFor(() => {
      expect(preferences).toHaveBeenCalledTimes(2);
      expect(setup).toHaveBeenCalledTimes(2);
      expect(microphones).toHaveBeenCalledTimes(2);
    });
    expect(screen.queryByRole("button", { name: "Retry preferences" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry setup" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry microphones" })).not.toBeInTheDocument();
  });

  it("reloads saved vocabulary after a native edit rejects", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
    vi.spyOn(bridge, "listSessions").mockResolvedValue([]);
    const saved = { id: "v1", spoken: "was api", replacement: "WASAPI", source: "manual" as const, uses: 2 };
    vi.spyOn(bridge, "listVocabulary").mockResolvedValue([saved]);
    vi.spyOn(bridge, "updateVocabulary").mockRejectedValue(new Error("vocabulary write failed"));
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Vocabulary" }));
    const correction = await screen.findByRole("textbox", { name: "Correction for was api" });
    fireEvent.change(correction, { target: { value: "Wrong local value" } });
    fireEvent.blur(correction);
    expect(await screen.findByRole("alert")).toHaveTextContent("vocabulary write failed");
    await waitFor(() => expect(screen.getByDisplayValue("WASAPI")).toBeInTheDocument());
    expect(screen.queryByDisplayValue("Wrong local value")).not.toBeInTheDocument();
  });
});
