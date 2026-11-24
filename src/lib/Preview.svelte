<script lang="ts">
  import { onMount } from "svelte";
  import {
    excludeCandidates,
    excludeMatching,
    formatWhen,
    listCandidates,
    plural,
    previewLabel,
    type Candidate,
    type Job,
  } from "./jobs";
  import { jobsStore } from "./jobsStore.svelte";

  let { scan, onclose }: { scan: Job; onclose: () => void } = $props();

  const PAGE = 100;

  let dialog = $state<HTMLDialogElement>();
  let search = $state("");
  let items = $state<Candidate[]>([]);
  let total = $state(0); // matching the search
  let included = $state(0); // will be deleted (whole list)
  let loading = $state(true);
  let error = $state("");
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function load(reset: boolean) {
    loading = true;
    error = "";
    try {
      const page = await listCandidates(scan.id, reset ? 0 : items.length, PAGE, search.trim() || null);
      items = reset ? page.items : [...items, ...page.items];
      total = page.total;
      included = page.included;
      jobsStore.setIncluded(scan.id, page.included);
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    dialog?.showModal();
    load(true);
    return () => clearTimeout(timer);
  });

  function onSearchInput() {
    clearTimeout(timer);
    timer = setTimeout(() => load(true), 250);
  }

  async function toggle(c: Candidate) {
    const next = !c.excluded;
    c.excluded = next; // optimistic
    try {
      included = await excludeCandidates(scan.id, [c.message_id], next);
      jobsStore.setIncluded(scan.id, included);
    } catch (e) {
      c.excluded = !next;
      error = String(e);
    }
  }

  async function setAllShown(excluded: boolean) {
    error = "";
    try {
      included = await excludeMatching(scan.id, search.trim() || null, excluded);
      jobsStore.setIncluded(scan.id, included);
      await load(true);
    } catch (e) {
      error = String(e);
    }
  }

  const where = $derived(`${scan.location} › ${scan.channel_name}`);
  const searching = $derived(search.trim() !== "");
</script>

<dialog bind:this={dialog} {onclose} aria-labelledby="review-title">
  <header>
    <h3 id="review-title">Review messages</h3>
    <p class="muted small">{where}</p>
    <p class="small">
      These are <strong>your own</strong> messages that matched the scan. Untick any you want to keep: only ticked
      messages are deleted.
    </p>
  </header>

  <div class="tools">
    <input
      type="search"
      placeholder="Search these messages…"
      aria-label="Search messages"
      bind:value={search}
      oninput={onSearchInput}
    />
    <button class="ghost" onclick={() => setAllShown(false)} title="Tick every message shown by the search">
      Tick {searching ? "matching" : "all"}
    </button>
    <button class="ghost" onclick={() => setAllShown(true)} title="Untick every message shown by the search">
      Untick {searching ? "matching" : "all"}
    </button>
  </div>

  {#if error}<p class="error small" role="alert">{error}</p>{/if}

  <ul class="items" aria-label="Messages found">
    {#each items as c (c.message_id)}
      <li>
        <label class:off={c.excluded}>
          <input
            type="checkbox"
            checked={!c.excluded}
            disabled={c.state !== "pending"}
            aria-label={`Delete this message: ${previewLabel(c)}`}
            onchange={() => toggle(c)}
          />
          <span class="when">{formatWhen(c.ts)}</span>
          <span class="text">{previewLabel(c)}</span>
          {#if c.attachments > 0 && (c.preview ?? "").trim()}
            <span class="badge">{plural(c.attachments, "file")}</span>
          {/if}
        </label>
      </li>
    {:else}
      <li class="muted pad">{loading ? "Loading…" : searching ? "No messages match your search." : "No messages."}</li>
    {/each}
  </ul>

  {#if items.length < total}
    <button class="ghost more" disabled={loading} onclick={() => load(false)}>
      Show more ({(total - items.length).toLocaleString("en-US")} left)
    </button>
  {/if}

  <footer>
    <span aria-live="polite">
      <strong>{included.toLocaleString("en-US")}</strong> of {plural(scan.found, "message")} will be deleted
    </span>
    <button class="primary" onclick={() => dialog?.close()}>Done</button>
  </footer>
</dialog>

<style>
  dialog { background: var(--card); color: var(--text); border: 1px solid var(--line); border-radius: 12px; padding: 18px; width: min(680px, 92vw); }
  dialog::backdrop { background: rgba(0, 0, 0, 0.6); }
  h3 { margin: 0 0 2px; }
  p { margin: 4px 0; }
  .muted { color: var(--muted); }
  .small { font-size: 13px; }
  .error { color: var(--danger); }
  .tools { display: flex; gap: 8px; margin: 12px 0 8px; flex-wrap: wrap; }
  .tools input { flex: 1; min-width: 160px; font: inherit; padding: 7px 10px; border-radius: 8px; border: 1px solid var(--line); background: var(--bg); color: var(--text); }
  .items { list-style: none; margin: 0; padding: 0; max-height: 46vh; overflow: auto; border: 1px solid var(--line); border-radius: 10px; background: var(--bg); }
  .items li { border-bottom: 1px solid var(--line); }
  .items li:last-child { border-bottom: 0; }
  .items label { display: flex; gap: 10px; align-items: baseline; padding: 8px 10px; cursor: pointer; }
  .items label.off .text { text-decoration: line-through; color: var(--muted); }
  .items input { accent-color: var(--accent); width: 16px; height: 16px; flex: none; align-self: center; }
  .when { color: var(--muted); font-size: 12px; white-space: nowrap; flex: none; }
  .text { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .badge { font-size: 11px; padding: 1px 6px; border-radius: 99px; border: 1px solid var(--line); color: var(--muted); flex: none; }
  .pad { padding: 14px; }
  .more { margin-top: 8px; width: 100%; }
  footer { display: flex; align-items: center; justify-content: space-between; margin-top: 14px; }
  button { font: inherit; border-radius: 8px; padding: 7px 14px; border: 1px solid var(--line); background: var(--card); color: var(--text); cursor: pointer; }
  button.ghost { background: transparent; }
  button.primary { background: var(--accent); border-color: var(--accent); color: #fff; }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
