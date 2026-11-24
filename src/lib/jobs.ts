import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type JobKind = "scan" | "delete";
export type JobStatus = "queued" | "running" | "paused" | "completed" | "failed" | "cancelled";
export type Speed = "gentle" | "normal";

export interface Job {
  id: number;
  kind: JobKind;
  channel_id: string;
  channel_name: string;
  location: string;
  status: JobStatus;
  author_id: string;
  options: string;
  cursor: string | null;
  scanned: number;
  found: number;
  deleted: number;
  failed: number;
  skipped: number;
  error: string | null;
  created_at: number;
  updated_at: number;
  guild_id: string | null;
  /** For a delete job: the scan whose reviewed list it deletes. */
  source_scan_id: number | null;
}

/** What a scan collects. All set conditions must hold. Times are ms since the Unix epoch. */
export interface Filters {
  after_ms?: number | null;
  before_ms?: number | null;
  contains?: string | null;
  only_with_attachments?: boolean;
  only_with_links?: boolean;
}

export interface ChannelRef {
  id: string;
  name: string;
  location: string;
  guild_id?: string | null;
}

export interface CreateScans {
  kind: "scan";
  channels: ChannelRef[];
  speed?: Speed;
  skip_pinned?: boolean;
  filters?: Filters;
}

export interface CreateDeletes {
  kind: "delete";
  scan_ids: number[];
  speed?: Speed;
  confirm: string;
}

export type CreateJobs = CreateScans | CreateDeletes;

export interface Candidate {
  message_id: string;
  ts: number;
  preview: string | null;
  attachments: number;
  excluded: boolean;
  state: "pending" | "deleted" | "skipped" | "failed";
}

export interface CandidatePage {
  /** Messages matching the search. */
  total: number;
  /** Messages that will be deleted (whole list, ignoring the search). */
  included: number;
  items: Candidate[];
}

export interface Protected {
  kind: "channel" | "guild";
  id: string;
  name: string;
}

/** The word a user must type to start a job that deletes messages. */
export const DELETE_CONFIRMATION = "DELETE";

// ------------------------------------------------------------ backend calls

export const listJobs = () => invoke<Job[]>("jobs_list");
export const createJobs = (req: CreateJobs) => invoke<Job[]>("jobs_create", { req });
export const pauseJob = (id: number) => invoke<void>("job_pause", { id });
export const resumeJob = (id: number) => invoke<void>("job_resume", { id });
export const cancelJob = (id: number) => invoke<void>("job_cancel", { id });
export const removeJob = (id: number) => invoke<void>("job_remove", { id });

export const listCandidates = (id: number, offset: number, limit: number, search: string | null) =>
  invoke<CandidatePage>("job_candidates", { id, offset, limit, search });
/** Returns how many messages will now be deleted. */
export const excludeCandidates = (id: number, messageIds: string[], excluded: boolean) =>
  invoke<number>("job_candidates_exclude", { id, messageIds, excluded });
export const excludeMatching = (id: number, search: string | null, excluded: boolean) =>
  invoke<number>("job_candidates_exclude_matching", { id, search, excluded });

export const listProtected = () => invoke<Protected[]>("protect_list");
export const addProtected = (kind: Protected["kind"], id: string, name: string) =>
  invoke<void>("protect_add", { kind, id, name });
export const removeProtected = (kind: Protected["kind"], id: string) => invoke<void>("protect_remove", { kind, id });

export const onJobUpdated = (cb: (job: Job) => void): Promise<UnlistenFn> =>
  listen<Job>("job-updated", (e) => cb(e.payload));

// ----------------------------------------------------------- pure helpers

export const canPause = (j: Job) => j.status === "queued" || j.status === "running";
export const canResume = (j: Job) => j.status === "paused";
export const canCancel = (j: Job) => j.status === "queued" || j.status === "running" || j.status === "paused";
export const isFinished = (j: Job) => j.status === "completed" || j.status === "failed" || j.status === "cancelled";
export const isActive = (j: Job) => j.status === "queued" || j.status === "running";

export function plural(n: number, one: string, many = one + "s"): string {
  return `${n.toLocaleString("en-US")} ${n === 1 ? one : many}`;
}

export function statusLabel(j: Job): string {
  const base: Record<JobStatus, string> = {
    queued: "Waiting",
    running: j.kind === "scan" ? "Scanning" : "Deleting",
    paused: "Paused",
    completed: "Done",
    failed: "Failed",
    cancelled: "Cancelled",
  };
  return base[j.status];
}

export function progressText(j: Job): string {
  const n = (x: number) => x.toLocaleString("en-US");
  const parts: string[] = [];
  if (j.kind === "scan") {
    parts.push(`${plural(j.found, "message")} found`);
    parts.push(`${n(j.scanned)} checked`);
  } else {
    parts.push(`${n(j.deleted)} of ${plural(j.found, "message")} deleted`);
    if (j.skipped > 0) parts.push(`${n(j.skipped)} already gone`);
    if (j.failed > 0) parts.push(`${n(j.failed)} failed`);
  }
  return parts.join(" · ");
}

/** 0..1 for a delete job (known total); `null` for a scan (total unknown). */
export function progressFraction(j: Job): number | null {
  if (j.kind !== "delete" || j.found <= 0) return null;
  return Math.min(1, (j.deleted + j.skipped + j.failed) / j.found);
}

export interface ParsedOptions {
  speed: Speed;
  skip_pinned: boolean;
  filters: Filters;
}

export function parseOptions(j: Pick<Job, "options">): ParsedOptions {
  try {
    const o = JSON.parse(j.options) as { speed?: Speed; skip_pinned?: boolean; filters?: Filters };
    return {
      speed: o.speed === "normal" ? "normal" : "gentle",
      skip_pinned: o.skip_pinned !== false,
      filters: o.filters ?? {},
    };
  } catch {
    return { speed: "gentle", skip_pinned: true, filters: {} };
  }
}

/**
 * Finished scans the user can review and delete from: they found something, no
 * delete job is based on them, and no newer finished scan of the same channel
 * supersedes them (the newest scan is the truth; older lists could be stale).
 */
export function deletableScans(jobs: Job[]): Job[] {
  return jobs
    .filter(
      (s) =>
        s.kind === "scan" &&
        s.status === "completed" &&
        s.found > 0 &&
        !jobs.some((d) => d.kind === "delete" && d.source_scan_id === s.id && d.status !== "cancelled" && d.status !== "failed") &&
        !jobs.some((n) => n.kind === "scan" && n.status === "completed" && n.channel_id === s.channel_id && n.id > s.id),
    )
    .sort((a, b) => a.id - b.id);
}

export function toChannelRef(c: { id: string; name: string; where: string; guildId?: string | null }): ChannelRef {
  return { id: c.id, name: c.name, location: c.where, guild_id: c.guildId ?? null };
}

/** Newest first. */
export function sortJobs(jobs: Job[]): Job[] {
  return [...jobs].sort((a, b) => b.id - a.id);
}

// ------------------------------------------------------------------ filters

export interface FilterForm {
  /** YYYY-MM-DD from a date input, or "". Messages on or after this day. */
  after: string;
  /** YYYY-MM-DD, or "". Messages up to and including this day. */
  before: string;
  contains: string;
  attachments: boolean;
  links: boolean;
}

export const emptyFilterForm = (): FilterForm => ({ after: "", before: "", contains: "", attachments: false, links: false });

/** Local midnight at the start of a YYYY-MM-DD date; `null` for anything else. */
export function startOfDayMs(date: string): number | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!m) return null;
  const [y, mo, day] = [Number(m[1]), Number(m[2]), Number(m[3])];
  const d = new Date(y, mo - 1, day);
  // JS rolls impossible dates over (Feb 30 -> Mar 1); only accept real calendar days.
  if (d.getFullYear() !== y || d.getMonth() !== mo - 1 || d.getDate() !== day) return null;
  return d.getTime();
}

/** The form's filters in the shape the backend expects. "Before" includes that whole day. */
export function filtersFromForm(f: FilterForm): Filters {
  const after = startOfDayMs(f.after);
  const beforeStart = startOfDayMs(f.before);
  let before: number | null = null;
  if (beforeStart !== null) {
    const next = new Date(beforeStart);
    next.setDate(next.getDate() + 1); // local next day (handles DST)
    before = next.getTime();
  }
  const out: Filters = {};
  if (after !== null) out.after_ms = after;
  if (before !== null) out.before_ms = before;
  const text = f.contains.trim();
  if (text) out.contains = text;
  if (f.attachments) out.only_with_attachments = true;
  if (f.links) out.only_with_links = true;
  return out;
}

/** An error message if the form can't be used, else `null`. */
export function filterFormError(f: FilterForm): string | null {
  const a = startOfDayMs(f.after);
  const b = startOfDayMs(f.before);
  if (f.after && a === null) return "The start date isn't valid.";
  if (f.before && b === null) return "The end date isn't valid.";
  if (a !== null && b !== null && a > b) return "The start date must not be after the end date.";
  return null;
}

const dayFmt = new Intl.DateTimeFormat("en-US", { year: "numeric", month: "short", day: "numeric" });

/** Short human phrases for the filters in use, e.g. ["from Jan 5, 2024", 'containing "pizza"']. */
export function describeFilters(f: Filters): string[] {
  const out: string[] = [];
  if (f.after_ms != null) out.push(`from ${dayFmt.format(new Date(f.after_ms))}`);
  if (f.before_ms != null) out.push(`until ${dayFmt.format(new Date(f.before_ms - 1))}`);
  if (f.contains) out.push(`containing "${f.contains}"`);
  if (f.only_with_attachments) out.push("with attachments");
  if (f.only_with_links) out.push("with links");
  return out;
}

export function hasFilters(f: Filters): boolean {
  return describeFilters(f).length > 0;
}

/** "Mar 3, 2024, 2:15 PM" in the user's locale. */
export function formatWhen(ts: number): string {
  return new Date(ts).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/** What to show for a message in the review list. */
export function previewLabel(c: Pick<Candidate, "preview" | "attachments">): string {
  const text = (c.preview ?? "").trim();
  if (text) return text;
  if (c.attachments > 0) return plural(c.attachments, "attachment").replace(/^/, "[") + "]";
  return "(no text)";
}
