<script lang="ts">
  import { onMount } from "svelte";
  import { getStatus, logout, onAuthChanged, onLoginFailed, startLogin } from "$lib/auth";
  import Background from "$lib/Background.svelte";
  import Browse from "$lib/Browse.svelte";
  import Jobs from "$lib/Jobs.svelte";
  import { displayName, initial, type CurrentUser } from "$lib/user";

  type View = "loading" | "signed-out" | "signed-in" | "offline";

  let view = $state<View>("loading");
  let user = $state<CurrentUser | null>(null);
  let error = $state("");
  let acknowledged = $state(false);
  let opening = $state(false);

  function apply(u: CurrentUser | null) {
    user = u;
    view = u ? "signed-in" : "signed-out";
    opening = false;
    if (u) error = "";
  }

  async function refresh() {
    view = "loading";
    try {
      apply(await getStatus());
    } catch (e) {
      error = String(e);
      view = "offline";
    }
  }

  async function signIn() {
    error = "";
    opening = true;
    try {
      await startLogin();
    } catch (e) {
      error = String(e);
      opening = false;
    }
  }

  async function signOut() {
    try {
      await logout();
    } catch (e) {
      error = String(e);
    }
  }

  onMount(() => {
    const unlisten = Promise.all([
      onAuthChanged(apply),
      onLoginFailed((m) => {
        error = m;
        opening = false;
      }),
    ]);
    refresh();
    return () => {
      unlisten.then((fns) => fns.forEach((f) => f()));
    };
  });
</script>

<main>
  <header>
    <h1>Concord</h1>
    <p class="tag">Bulk-delete your own Discord messages</p>
  </header>

  {#if view === "loading"}
    <p class="muted" role="status">Checking your session…</p>
  {:else if view === "offline"}
    <section class="card">
      <h2>Can't reach Discord</h2>
      <p class="error" role="alert">{error}</p>
      <button onclick={refresh}>Try again</button>
    </section>
  {:else if view === "signed-in" && user}
    <section class="card account">
      <div class="avatar" aria-hidden="true">{initial(user)}</div>
      <div class="who">
        <strong>{displayName(user)}</strong>
        <span class="muted">@{user.username}</span>
      </div>
      <button class="secondary" onclick={signOut}>Sign out</button>
    </section>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <Background />
    <Browse />
    <Jobs />
  {:else}
    <section class="card warn" aria-labelledby="risk">
      <h2 id="risk">Read this first</h2>
      <p>
        Concord signs in as <em>you</em> and calls Discord the way its own app does. <strong>Discord does
        not allow this.</strong> Automating a user account can get it limited or permanently terminated.
        Use it at your own risk, ideally with an account you can afford to lose.
      </p>
      <p>
        Your login token is a full key to your account. Concord keeps it in your operating system's
        keychain only. It is never shown, logged, or sent anywhere except to Discord.
      </p>
      <label class="check">
        <input type="checkbox" bind:checked={acknowledged} />
        I understand the risk and that this is not supported by Discord.
      </label>
    </section>

    <button class="primary" disabled={!acknowledged || opening} onclick={signIn}>
      {opening ? "Waiting for sign-in window…" : "Sign in with Discord"}
    </button>
    <p class="muted small">
      A Discord sign-in window opens. Easiest: scan the QR code on it with your Discord phone app
      (Settings → Scan QR Code). Concord never sees your password.
    </p>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
  {/if}
</main>

<style>
  :global(:root) {
    color-scheme: dark;
    --bg: #14151a;
    --card: #1e2028;
    --line: #2d303b;
    --text: #e6e8ee;
    --muted: #9aa0ae;
    --accent: #5865f2;
    --warn: #f0b429;
    --danger: #ff6b6b;
  }
  :global(#app-root) {
    /* The wrapper added by app.html. A class + rule instead of an inline style, which the CSP forbids. */
    display: contents;
  }
  :global(body) {
    margin: 0;
    background: var(--bg);
    color: var(--text);
    font: 15px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif;
  }
  main {
    max-width: 760px;
    margin: 0 auto;
    padding: 40px 24px;
  }
  h1 { margin: 0; font-size: 28px; }
  h2 { margin: 0 0 8px; font-size: 17px; }
  .tag, .muted { color: var(--muted); }
  .small { font-size: 13px; }
  header { margin-bottom: 28px; }
  .card {
    background: var(--card);
    border: 1px solid var(--line);
    border-radius: 12px;
    padding: 18px 20px;
    margin-bottom: 16px;
  }
  .warn { border-color: var(--warn); }
  .check { display: flex; gap: 10px; align-items: flex-start; margin-top: 12px; cursor: pointer; }
  .check input { margin-top: 4px; accent-color: var(--accent); }
  button {
    font: inherit;
    border-radius: 8px;
    padding: 10px 18px;
    border: 1px solid var(--line);
    background: var(--card);
    color: var(--text);
    cursor: pointer;
  }
  button.primary { background: var(--accent); border-color: var(--accent); color: #fff; width: 100%; }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .account { display: flex; align-items: center; gap: 14px; }
  .avatar {
    width: 44px; height: 44px; border-radius: 50%;
    background: var(--accent); display: grid; place-items: center;
    font-weight: 700; font-size: 18px;
  }
  .who { display: flex; flex-direction: column; flex: 1; min-width: 0; }
  .error { color: var(--danger); }
</style>
