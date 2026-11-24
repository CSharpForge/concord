import { listCandidates, listJobs, onJobUpdated, sortJobs, type Job } from "./jobs";

/** Live view of all jobs, kept current by `job-updated` events from the backend. */
class JobsStore {
  private byId = $state<Record<number, Job>>({});
  loaded = $state(false);
  /** For finished scans: how many messages will be deleted (found minus any the user excluded). */
  included = $state<Record<number, number>>({});

  get list(): Job[] {
    return sortJobs(Object.values(this.byId));
  }

  put(job: Job): void {
    this.byId = { ...this.byId, [job.id]: job };
  }

  drop(id: number): void {
    const { [id]: _gone, ...rest } = this.byId;
    this.byId = rest;
    const { [id]: _n, ...inc } = this.included;
    this.included = inc;
  }

  setIncluded(id: number, n: number): void {
    this.included = { ...this.included, [id]: n };
  }

  /** Ask the backend how many of a scan's messages are still selected for deletion. */
  async refreshIncluded(id: number): Promise<void> {
    const page = await listCandidates(id, 0, 1, null);
    this.setIncluded(id, page.included);
  }

  /** Load existing jobs, then follow updates. Returns a function that stops following. */
  async start(): Promise<() => void> {
    const unlisten = await onJobUpdated((j) => this.put(j));
    const existing = await listJobs();
    // An update that arrived while loading is newer than the list: keep it.
    const merged: Record<number, Job> = {};
    for (const j of existing) merged[j.id] = j;
    for (const j of Object.values(this.byId)) if (!merged[j.id] || j.updated_at >= merged[j.id].updated_at) merged[j.id] = j;
    this.byId = merged;
    this.loaded = true;
    return unlisten;
  }
}

export const jobsStore = new JobsStore();
