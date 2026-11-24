import { describe, expect, it } from "vitest";
import {
  canCancel,
  canPause,
  canResume,
  deletableScans,
  describeFilters,
  emptyFilterForm,
  filterFormError,
  filtersFromForm,
  formatWhen,
  hasFilters,
  isFinished,
  parseOptions,
  plural,
  previewLabel,
  progressFraction,
  progressText,
  sortJobs,
  startOfDayMs,
  statusLabel,
  toChannelRef,
  type Job,
} from "./jobs";

let nextId = 1;
const job = (over: Partial<Job> = {}): Job => ({
  id: nextId++,
  kind: "scan",
  channel_id: "500",
  channel_name: "general",
  location: "Zebra Club",
  status: "completed",
  author_id: "42",
  options: "{}",
  cursor: null,
  scanned: 0,
  found: 0,
  deleted: 0,
  failed: 0,
  skipped: 0,
  error: null,
  created_at: 0,
  updated_at: 0,
  guild_id: null,
  source_scan_id: null,
  ...over,
});

describe("plural", () => {
  it("handles 1 and many, with thousands separators", () => {
    expect(plural(1, "message")).toBe("1 message");
    expect(plural(0, "message")).toBe("0 messages");
    expect(plural(12345, "message")).toBe("12,345 messages");
  });
});

describe("status helpers", () => {
  it("controls match the lifecycle", () => {
    expect(canPause(job({ status: "running" }))).toBe(true);
    expect(canPause(job({ status: "paused" }))).toBe(false);
    expect(canResume(job({ status: "paused" }))).toBe(true);
    expect(canCancel(job({ status: "paused" }))).toBe(true);
    expect(canCancel(job({ status: "completed" }))).toBe(false);
    expect(isFinished(job({ status: "failed" }))).toBe(true);
    expect(isFinished(job({ status: "queued" }))).toBe(false);
  });
  it("labels running jobs by kind", () => {
    expect(statusLabel(job({ kind: "scan", status: "running" }))).toBe("Scanning");
    expect(statusLabel(job({ kind: "delete", status: "running" }))).toBe("Deleting");
    expect(statusLabel(job({ status: "queued" }))).toBe("Waiting");
  });
});

describe("progress", () => {
  it("describes a scan", () => {
    expect(progressText(job({ kind: "scan", found: 3, scanned: 1200 }))).toBe("3 messages found · 1,200 checked");
    expect(progressText(job({ kind: "scan", found: 1 }))).toContain("1 message found");
  });
  it("describes a delete against its known total, mentioning skipped/failed only when non-zero", () => {
    expect(progressText(job({ kind: "delete", found: 8, deleted: 5 }))).toBe("5 of 8 messages deleted");
    expect(progressText(job({ kind: "delete", found: 1, deleted: 1 }))).toBe("1 of 1 message deleted");
    expect(progressText(job({ kind: "delete", found: 9, deleted: 4, skipped: 2, failed: 3 }))).toBe(
      "4 of 9 messages deleted · 2 already gone · 3 failed",
    );
  });
  it("has a fraction only for deletes with a known total", () => {
    expect(progressFraction(job({ kind: "delete", found: 10, deleted: 3, skipped: 1, failed: 1 }))).toBe(0.5);
    expect(progressFraction(job({ kind: "delete", found: 0 }))).toBeNull();
    expect(progressFraction(job({ kind: "scan", found: 10 }))).toBeNull();
    expect(progressFraction(job({ kind: "delete", found: 2, deleted: 5 }))).toBe(1); // never above 1
  });
});

describe("parseOptions", () => {
  it("reads speed, skip_pinned and filters, with safe defaults", () => {
    expect(parseOptions({ options: '{"speed":"normal","skip_pinned":false,"filters":{"contains":"x"}}' })).toEqual({
      speed: "normal",
      skip_pinned: false,
      filters: { contains: "x" },
    });
    expect(parseOptions({ options: "{}" })).toEqual({ speed: "gentle", skip_pinned: true, filters: {} });
    expect(parseOptions({ options: "not json" })).toEqual({ speed: "gentle", skip_pinned: true, filters: {} });
    expect(parseOptions({ options: '{"speed":"turbo"}' }).speed).toBe("gentle");
  });
});

describe("deletableScans", () => {
  it("returns completed scans that found something", () => {
    const a = job({ found: 4 });
    const none = job({ channel_id: "501", found: 0 });
    const running = job({ channel_id: "502", found: 9, status: "running" });
    expect(deletableScans([a, none, running]).map((j) => j.id)).toEqual([a.id]);
  });
  it("hides a scan once a delete job is based on it, but not if that delete was cancelled or failed", () => {
    const scan = job({ found: 2 });
    for (const status of ["queued", "running", "paused", "completed"] as const) {
      expect(deletableScans([scan, job({ kind: "delete", source_scan_id: scan.id, status })])).toEqual([]);
    }
    for (const status of ["cancelled", "failed"] as const) {
      expect(deletableScans([scan, job({ kind: "delete", source_scan_id: scan.id, status })]).map((j) => j.id)).toEqual([scan.id]);
    }
  });
  it("a delete based on a DIFFERENT scan of the same channel does not hide it", () => {
    const old = job({ channel_id: "700", found: 1 });
    const del = job({ kind: "delete", channel_id: "700", source_scan_id: old.id, status: "completed" });
    const fresh = job({ channel_id: "700", found: 3 });
    expect(deletableScans([old, del, fresh]).map((j) => j.id)).toEqual([fresh.id]);
  });
});

describe("deletableScans: newest scan per channel wins", () => {
  it("hides an older finished scan once a newer finished scan of the same channel exists", () => {
    const old = job({ channel_id: "800", found: 5 });
    const fresh = job({ channel_id: "800", found: 2 });
    expect(deletableScans([old, fresh]).map((j) => j.id)).toEqual([fresh.id]);
  });
  it("a newer scan that found nothing still supersedes the older list", () => {
    const old = job({ channel_id: "800", found: 5 });
    const empty = job({ channel_id: "800", found: 0 });
    expect(deletableScans([old, empty])).toEqual([]);
  });
  it("a newer scan that is still running does not hide the older one yet", () => {
    const old = job({ channel_id: "800", found: 5 });
    const running = job({ channel_id: "800", status: "running" });
    expect(deletableScans([old, running]).map((j) => j.id)).toEqual([old.id]);
  });
  it("scans of different channels are independent", () => {
    const a = job({ channel_id: "1", found: 1 });
    const b = job({ channel_id: "2", found: 1 });
    expect(deletableScans([a, b]).map((j) => j.id)).toEqual([a.id, b.id]);
  });
});

describe("dates and filters", () => {
  it("startOfDayMs is local midnight, and rejects invalid or impossible dates", () => {
    expect(startOfDayMs("2024-01-05")).toBe(new Date(2024, 0, 5).getTime());
    expect(startOfDayMs("")).toBeNull();
    expect(startOfDayMs("2024-1-5")).toBeNull();
    expect(startOfDayMs("2024-02-30")).toBeNull(); // would roll over to Mar 1
    expect(startOfDayMs("2024-13-01")).toBeNull();
    expect(startOfDayMs("2024-02-29")).not.toBeNull(); // 2024 is a leap year
    expect(startOfDayMs("2023-02-29")).toBeNull();
  });

  it("an empty form gives no filters", () => {
    expect(filtersFromForm(emptyFilterForm())).toEqual({});
  });

  it("converts the form: 'before' includes that whole day", () => {
    const f = filtersFromForm({ after: "2024-01-05", before: "2024-03-02", contains: "  pizza ", attachments: true, links: true });
    expect(f.after_ms).toBe(new Date(2024, 0, 5).getTime());
    expect(f.before_ms).toBe(new Date(2024, 2, 3).getTime()); // start of the NEXT day (exclusive bound)
    expect(f.contains).toBe("pizza");
    expect(f.only_with_attachments).toBe(true);
    expect(f.only_with_links).toBe(true);
  });

  it("omits unset options instead of sending false/empty values", () => {
    expect(filtersFromForm({ ...emptyFilterForm(), contains: "   " })).toEqual({});
    expect(filtersFromForm({ ...emptyFilterForm(), after: "2024-01-05" })).toEqual({ after_ms: new Date(2024, 0, 5).getTime() });
  });

  it("validates the form", () => {
    expect(filterFormError(emptyFilterForm())).toBeNull();
    expect(filterFormError({ ...emptyFilterForm(), after: "2024-03-01", before: "2024-03-01" })).toBeNull(); // same day is fine
    expect(filterFormError({ ...emptyFilterForm(), after: "2024-03-02", before: "2024-03-01" })).toMatch(/must not be after/);
    expect(filterFormError({ ...emptyFilterForm(), after: "2024-02-30" })).toMatch(/start date isn't valid/);
    expect(filterFormError({ ...emptyFilterForm(), before: "garbage" })).toMatch(/end date isn't valid/);
  });

  it("describes active filters in plain words", () => {
    const f = filtersFromForm({ after: "2024-01-05", before: "2024-03-02", contains: "pizza", attachments: true, links: false });
    expect(describeFilters(f)).toEqual(["from Jan 5, 2024", "until Mar 2, 2024", 'containing "pizza"', "with attachments"]);
    expect(describeFilters({})).toEqual([]);
    expect(hasFilters({})).toBe(false);
    expect(hasFilters({ only_with_links: true })).toBe(true);
  });
});

describe("review list formatting", () => {
  it("previewLabel shows text, else attachments, else a placeholder", () => {
    expect(previewLabel({ preview: "hello", attachments: 0 })).toBe("hello");
    expect(previewLabel({ preview: "", attachments: 2 })).toBe("[2 attachments]");
    expect(previewLabel({ preview: null, attachments: 1 })).toBe("[1 attachment]");
    expect(previewLabel({ preview: "  ", attachments: 0 })).toBe("(no text)");
    expect(previewLabel({ preview: null, attachments: 0 })).toBe("(no text)");
  });
  it("formatWhen produces a readable local date and time", () => {
    const s = formatWhen(new Date(2024, 2, 3, 14, 15).getTime());
    expect(s).toMatch(/2024/);
    expect(s.length).toBeGreaterThan(8);
  });
});

describe("misc", () => {
  it("toChannelRef maps selection fields incl. the server id", () => {
    expect(toChannelRef({ id: "9", name: "n", where: "L", guildId: "7" })).toEqual({ id: "9", name: "n", location: "L", guild_id: "7" });
    expect(toChannelRef({ id: "9", name: "n", where: "L", guildId: null }).guild_id).toBeNull();
    expect(toChannelRef({ id: "9", name: "n", where: "L" }).guild_id).toBeNull();
  });
  it("sortJobs is newest first and non-mutating", () => {
    const a = job(),
      b = job();
    const input = [a, b];
    expect(sortJobs(input).map((j) => j.id)).toEqual([b.id, a.id]);
    expect(input[0]).toBe(a);
  });
});
