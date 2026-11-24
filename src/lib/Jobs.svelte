<script lang="ts">
  import { onMount } from "svelte";
  import {
    DELETE_CONFIRMATION,
    cancelJob,
    canCancel,
    canPause,
    canResume,
    createJobs,
    deletableScans,
    describeFilters,
    emptyFilterForm,
    filterFormError,
    filtersFromForm,
    isActive,
    isFinished,
    parseOptions,
    pauseJob,
    plural,
    progressFraction,
    progressText,
    removeJob,
    resumeJob,
    statusLabel,
    toChannelRef,
    type Job,
    type Speed,
  } from "./jobs";
  import { jobsStore } from "./jobs.svelte";
  import Preview from "./Preview.svelte";
  import { selection } from "./selection.svelte";

  let speed = $state<Speed>("gentle");
  let skipPinned = $state(true);
  let form = $state(emptyFilterForm());
  let busy = $state(false);
  let error = $state("");

  // Review dialog: the scan being reviewed.
  let reviewing = $state<Job | null>(null);
  // Delete confirmation: the scans the user is about to delete from.
  let confirmJobs = $state<Job[] | null>(null);
  let confirmText = $state("");
  let dialog = $state<HTMLDialogElement>();

  const jobs = $derived(jobsStore.list);
  const formError = $derived(filterFormError(form));
  const activeFilters = $derived(describeFilters(filtersFromForm(form)));

  /** How many messages of a scan will be deleted (found, minus any the user unticked). */
  const countFor = (j: Job) => jobsStore.included[j.id] ?? j.found;

  /** Finished scans with something to review (even if the user has unticked everything). */
  const reviewable = $derived(deletableScans(jobs));
  /** ...of which at least one message is still ticked for deletion. */
  const deletable = $derived(reviewable.filter((j) => countFor(j) > 0));
  const deletableTotal = $derived(deletable.reduce((n, j) => n + countFor(j), 0));
  const confirmTotal = $derived((confirmJobs ?? []).reduce((n, j) => n + countFor(j), 0));
  const activeCount = $derived(jobs.filter(isActive).length);

  $effect(() => {
    if (!dialog) return;
    if (confirmJobs && !dialog.open) dialog.showModal();
    if (!confirmJobs && dialog.open) dialog.close();
  });

  // Learn each finished scan's current "will be deleted" count.
  $effect(() => {
    for (const j of reviewable) {
      if (jobsStore.included[j.id] === undefined) jobsStore.refreshIncluded(j.id).catch(() => {});
    }
  });

  onMount(() => {
    let stop: (() => void) | undefined;
    let unmounted = false;
    jobsStore
      .start()
      .then((f) => (unmounted ? f() : (stop = f)))
      .catch((e) => (error = String(e)));
    return () => {
      unmounted = true;
      stop?.();
    };
  });

  async function run<T>(fn: () => Promise<T>): Promise<T | undefined> {
    error = "";
    busy = true;
    try {
      return await fn();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  const scanSelected = () =>
    run(() =>
      createJobs({
        kind: "scan",
        channels: [...selection.items.values()].map(toChannelRef),
        speed,
        skip_pinned: skipPinned,
        filters: filtersFromForm(form),
      }),
    );

  function askDelete(targets: Job[]) {
    confirmText = "";
    confirmJobs = targets;
  }

  async function confirmDelete() {
    const targets = confirmJobs;
    if (!targets || confirmText !== DELETE_CONFIRMATION) return;
    const ok = await run(async () => {
      // The backend deletes exactly each scan's reviewed list, with that scan's own settings.
      await createJobs({ kind: "delete", scan_ids: targets.map((j) => j.id), confirm: confirmText });
      return true;
    });
    if (ok) confirmJobs = null;
  }

  function closeConfirm() {
    confirmJobs = null;
    confirmText = "";
  }

  async function closeReview() {
    const j = reviewing;
    reviewing = null;
    if (j) await jobsStore.refreshIncluded(j.id).catch(() => {});
  }

  const where = (j: Job) => `${j.location} › ${j.channel_name}`;
  const act = (fn: (id: number) => Promise<void>, id: number) => run(() => fn(id));
  const canReview = (j: Job) => reviewable.some((d) => d.id === j.id);
  const canDeleteFrom = (j: Job) => deletable.some((d) => d.id === j.id);
  const chips = (j: Job) => {
    const o = parseOptions(j);
    return j.kind === "scan" ? describeFilters(o.filters) : [];
  };
</script>

<section class="panel" aria-label="Cleanup">
  <header>
    <h2>Cleanup</h2>
    <p class="muted small">
      <strong>1. Scan</strong> finds your own messages and deletes nothing. <strong>2. Review</strong> the list and untick
      anything to keep. <strong>3. Delete</strong> removes exactly what you reviewed.
    </p>
  </header>

  <div class="controls">
    <label>
      Speed
      <select bind:value={speed} aria-describedby="speed-help">
        <option value="gentle">Gentle (recommended)</option>
        <option value="normal">Normal (faster)</option>
      </select>
    </label>
    <label class="inline">
      <input type="checkbox" bind:checked={skipPinned} /> Keep pinned messages
    </label>
    <button class="primary" disabled={selection.count === 0 || busy || formError !== null} onclick={scanSelected}>
      {selection.count === 0 ? "Select channels to scan" : `Scan ${plural(selection.count, "channel")}`}
    </button>
  </div>
  <p id="speed-help" class="muted small">
    Concord follows Discord's rate limits and slows down when asked to. Gentle is slower but safer.
  </p>

  <details class="filters">
    <summary>
      Filters
      {#if activeFilters.length > 0}<span class="muted small">— {activeFilters.join(", ")}</span>{:else}<span class="muted small">(optional: date range, words, attachments)</span>{/if}
    </summary>
    <div class="filter-grid">
      <label>From <input type="date" bind:value={form.after} aria-label="From date" /></label>
      <label>To (inclusive) <input type="date" bind:value={form.before} aria-label="To date" /></label>
      <label class="wide">
        Containing
        <input type="text" bind:value={form.contains} placeholder="a word or phrase" aria-label="Containing text" autocomplete="off" />
      </label>
      <label class="inline"><input type="checkbox" bind:checked={form.attachments} /> Only with attachments</label>
      <label class="inline"><input type="checkbox" bind:checked={form.links} /> Only with links</label>
      <button class="ghost" onclick={() => (form = emptyFilterForm())} disabled={activeFilters.length === 0}>Clear filters</button>
    </div>
    {#if formError}<p class="error small" role="alert">{formError}</p>{/if}
  </details>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if deletable.length > 0}
    <div class="callout">
      <span>
        Scan results: <strong>{plural(deletableTotal, "message")}</strong> of yours in
        {plural(deletable.length, "channel")} are ready to review.
      </span>
      <button class="danger" disabled={busy} onclick={() => askDelete(deletable)}>
        Delete {deletableTotal.toLocaleString("en-US")}…
      </button>
    </div>
  {/if}

  {#if jobs.length === 0}
    <p class="muted pad">{jobsStore.loaded ? "No cleanup jobs yet." : "Loading…"}</p>
  {:else}
    <ul class="jobs" aria-label="Jobs">
      {#each jobs as j (j.id)}
        {@const fraction = progressFraction(j)}
        <li class="job" data-status={j.status}>
          <div class="top">
            <span class="kind" class:del={j.kind === "delete"}>{j.kind === "scan" ? "SCAN" : "DELETE"}</span>
            <span class="where" title={where(j)}>{where(j)}</span>
            <span class="status s-{j.status}">{statusLabel(j)}</span>
          </div>
          {#if j.status === "running"}
            {#if fraction === null}
              <div class="bar" role="progressbar" aria-label="In progress"></div>
            {:else}
              <div class="bar det" role="progressbar" aria-label="Deleting" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(fraction * 100)}>
                <span style="width: {fraction * 100}%"></span>
              </div>
            {/if}
          {/if}
          <div class="meta muted small">{progressText(j)}</div>
          {#if chips(j).length > 0}
            <div class="chips">{#each chips(j) as c}<span class="chip">{c}</span>{/each}</div>
          {/if}
          {#if j.error}<div class="error small">{j.error}</div>{/if}
          <div class="actions">
            {#if canReview(j)}
              <button disabled={busy} onclick={() => (reviewing = j)}>Review {plural(j.found, "message")}…</button>
            {/if}
            {#if canDeleteFrom(j)}
              <button class="danger" disabled={busy} onclick={() => askDelete([j])}>Delete {countFor(j).toLocaleString("en-US")}…</button>
            {:else if canReview(j)}
              <span class="muted small">Nothing ticked: nothing will be deleted.</span>
            {/if}
            {#if canPause(j)}<button disabled={busy} onclick={() => act(pauseJob, j.id)}>Pause</button>{/if}
            {#if canResume(j)}<button disabled={busy} onclick={() => act(resumeJob, j.id)}>Resume</button>{/if}
            {#if canCancel(j)}<button class="ghost" disabled={busy} onclick={() => act(cancelJob, j.id)}>Cancel</button>{/if}
            {#if isFinished(j)}
              <button
                class="ghost"
                disabled={busy}
                title={j.kind === "scan" ? "Also erases the stored list of messages" : undefined}
                onclick={async () => {
                  if ((await act(removeJob, j.id)) !== undefined || !error) jobsStore.drop(j.id);
                }}>Remove</button
              >
            {/if}
          </div>
        </li>
      {/each}
    </ul>
    <p class="muted small pad">
      Scans keep a short preview of each message on this computer so you can review it; the text is erased once the
      deletion finishes or you remove the scan.
      {#if activeCount > 0}Jobs run one at a time.{/if}
    </p>
  {/if}
</section>

{#if reviewing}
  <Preview scan={reviewing} onclose={closeReview} />
{/if}

<dialog bind:this={dialog} onclose={closeConfirm} aria-labelledby="confirm-title">
  {#if confirmJobs}
    <h3 id="confirm-title">Permanently delete {plural(confirmTotal, "message")}?</h3>
    <p>
      This deletes the messages you reviewed from Discord. <strong>It cannot be undone</strong> and Concord keeps no copy.
    </p>
    <ul class="targets">
      {#each confirmJobs.slice(0, 6) as j (j.id)}
        <li>{where(j)} — {plural(countFor(j), "message")}</li>
      {/each}
      {#if confirmJobs.length > 6}<li>…and {confirmJobs.length - 6} more channels</li>{/if}
    </ul>
    <label class="confirm">
      Type <code>{DELETE_CONFIRMATION}</code> to confirm
      <input bind:value={confirmText} autocomplete="off" spellcheck="false" aria-label="Confirmation word" />
    </label>
    {#if error}<p class="error small" role="alert">{error}</p>{/if}
    <div class="dialog-actions">
      <button onclick={closeConfirm}>Cancel</button>
      <button class="danger" disabled={confirmText !== DELETE_CONFIRMATION || busy} onclick={confirmDelete}>
        Delete forever
      </button>
    </div>
  {/if}
</dialog>

<style>
  .panel { margin-top: 16px; border: 1px solid var(--line); border-radius: 12px; background: var(--card); padding: 14px 16px; }
  h2 { margin: 0 0 4px; font-size: 17px; }
  h3 { margin: 0 0 8px; }
  .controls { display: flex; gap: 14px; align-items: center; flex-wrap: wrap; margin: 12px 0 4px; }
  label { display: flex; gap: 8px; align-items: center; }
  .inline { cursor: pointer; }
  select, input:not([type="checkbox"]) { font: inherit; padding: 6px 8px; border-radius: 8px; border: 1px solid var(--line); background: var(--bg); color: var(--text); }
  input[type="checkbox"] { accent-color: var(--accent); width: 16px; height: 16px; }
  button { font: inherit; border-radius: 8px; padding: 7px 14px; border: 1px solid var(--line); background: var(--card); color: var(--text); cursor: pointer; }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  button.primary { background: var(--accent); border-color: var(--accent); color: #fff; margin-left: auto; }
  button.danger { background: #3a1d22; border-color: var(--danger); color: #ffb3b3; }
  button.ghost { background: transparent; }
  button:focus-visible, select:focus-visible, input:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .muted { color: var(--muted); }
  .small { font-size: 13px; }
  .pad { padding: 8px 0 0; margin: 0; }
  .error { color: var(--danger); }
  .filters { margin: 10px 0 4px; border: 1px solid var(--line); border-radius: 10px; padding: 8px 12px; background: var(--bg); }
  .filters summary { cursor: pointer; }
  .filter-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 10px 14px; margin-top: 10px; align-items: end; }
  .filter-grid label { flex-direction: column; align-items: stretch; gap: 4px; }
  .filter-grid label.inline { flex-direction: row; align-items: center; }
  .filter-grid .wide { grid-column: 1 / -1; }
  .callout { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin: 12px 0; padding: 10px 12px; border: 1px solid var(--warn); border-radius: 10px; background: #2a2412; }
  .jobs { list-style: none; margin: 12px 0 0; padding: 0; display: flex; flex-direction: column; gap: 8px; max-height: 40vh; overflow: auto; }
  .job { border: 1px solid var(--line); border-radius: 10px; padding: 10px 12px; background: var(--bg); }
  .top { display: flex; align-items: center; gap: 10px; }
  .kind { font: 700 10px/1 ui-monospace, monospace; padding: 3px 6px; border-radius: 4px; border: 1px solid var(--line); color: var(--muted); }
  .kind.del { color: var(--danger); border-color: var(--danger); }
  .where { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .status { font-size: 12px; padding: 2px 8px; border-radius: 99px; border: 1px solid var(--line); }
  .s-running { color: #8fb4ff; border-color: #8fb4ff; }
  .s-completed { color: #7ee2a8; border-color: #7ee2a8; }
  .s-failed { color: var(--danger); border-color: var(--danger); }
  .s-paused, .s-queued { color: var(--warn); border-color: var(--warn); }
  .meta { margin-top: 4px; }
  .chips { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 6px; }
  .chip { font-size: 12px; padding: 1px 8px; border-radius: 99px; border: 1px solid var(--line); color: var(--muted); }
  .actions { display: flex; gap: 8px; margin-top: 8px; flex-wrap: wrap; }
  .actions:empty { display: none; }
  .bar { height: 3px; margin: 8px 0 2px; border-radius: 2px; background: var(--line); overflow: hidden; position: relative; }
  .bar:not(.det)::after { content: ""; position: absolute; inset: 0; width: 35%; background: var(--accent); animation: slide 1.4s ease-in-out infinite; }
  .bar.det span { display: block; height: 100%; background: var(--accent); transition: width 0.2s; }
  @keyframes slide { 0% { transform: translateX(-100%); } 100% { transform: translateX(300%); } }
  @media (prefers-reduced-motion: reduce) { .bar:not(.det)::after { animation: none; width: 100%; opacity: 0.5; } .bar.det span { transition: none; } }
  dialog { background: var(--card); color: var(--text); border: 1px solid var(--danger); border-radius: 12px; padding: 20px; max-width: 460px; }
  dialog::backdrop { background: rgba(0, 0, 0, 0.6); }
  .targets { margin: 8px 0 12px; padding-left: 18px; color: var(--muted); font-size: 13px; }
  .confirm { display: flex; flex-direction: column; align-items: stretch; gap: 6px; }
  code { background: var(--bg); padding: 1px 6px; border-radius: 4px; }
  .dialog-actions { display: flex; justify-content: flex-end; gap: 10px; margin-top: 16px; }
</style>
