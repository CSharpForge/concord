<script lang="ts">
  import { onMount } from "svelte";
  import { getBackgroundStatus, setAutostart } from "./background";

  let supported = $state(false);
  let autostart = $state(false);
  let busy = $state(false);
  let error = $state("");

  onMount(async () => {
    try {
      const s = await getBackgroundStatus();
      supported = s.supported;
      autostart = s.autostart;
    } catch (e) {
      error = String(e);
    }
  });

  async function toggle(event: Event) {
    const box = event.currentTarget as HTMLInputElement;
    const wanted = box.checked;
    busy = true;
    error = "";
    try {
      autostart = await setAutostart(wanted);
    } catch (e) {
      autostart = !wanted;
      error = String(e);
    } finally {
      // Set the element itself: if `autostart` didn't change (a failed enable
      // leaves it false) Svelte would not touch the box and it would stay ticked.
      box.checked = autostart;
      busy = false;
    }
  }
</script>

{#if supported}
  <section class="card" aria-label="Background">
    <h2>Background</h2>
    <label class="row">
      <input type="checkbox" checked={autostart} disabled={busy} onchange={toggle} />
      <span>Start Concord when I sign in to Windows</span>
    </label>
    <p class="muted small">
      It opens minimized in the system tray. Jobs that were running stay <strong>paused</strong> until you resume
      them, so nothing is deleted without you.
    </p>
    <p class="muted small">
      Closing the window keeps Concord running in the tray so jobs continue. To exit, right-click the tray icon and
      choose <strong>Quit Concord</strong>.
    </p>
    {#if error}<p class="error small" role="alert">{error}</p>{/if}
  </section>
{/if}

<style>
  .card { background: var(--card); border: 1px solid var(--line); border-radius: 12px; padding: 14px 18px; margin-bottom: 16px; }
  h2 { margin: 0 0 8px; font-size: 16px; }
  .row { display: flex; gap: 10px; align-items: center; cursor: pointer; }
  input[type="checkbox"] { accent-color: var(--accent); width: 16px; height: 16px; }
  input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  p { margin: 8px 0 0; }
  .muted { color: var(--muted); }
  .small { font-size: 13px; }
  .error { color: var(--danger); }
</style>
