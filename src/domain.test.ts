import { describe, expect, it } from "vitest";
import { expiryLabel, filterSessions, formatTimestamp } from "./domain";
import { sessions } from "./fixtures";

describe("session helpers", () => {
  it("searches Vietnamese text without changing its content", () => {
    const result = filterSessions(sessions, "tóm tắt");
    expect(result).toHaveLength(1);
    expect(result[0]?.title).toContain("Không sao");
  });

  it("searches raw transcript text", () => {
    expect(filterSessions(sessions, "friday friday")[0]?.id).toBe("dictation-20");
  });

  it("formats transcript citations", () => {
    expect(formatTimestamp(724)).toBe("12:04");
    expect(formatTimestamp(5)).toBe("0:05");
  });

  it("reports meeting expiry without negative values", () => {
    expect(expiryLabel("2026-09-30", new Date("2026-09-07"))).toBe("23 days left");
    expect(expiryLabel("2026-09-01", new Date("2026-09-07"))).toBe("Expires today");
  });
});
