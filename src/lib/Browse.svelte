<script lang="ts">
  import { onMount } from "svelte";
  import {
    KIND_LABEL,
    guildIconUrl,
    listDms,
    listGuildChannels,
    listGuilds,
    type ChannelInfo,
    type DmInfo,
    type GuildChannels,
    type GuildInfo,
  } from "./discover";
  import { checkState, filterGroups, matches } from "./filter";
  import { protectStore } from "./protect.svelte";
  import { selection, type SelectedChannel } from "./selection.svelte";

  type Tab = "servers" | "dms";

  let tab = $state<Tab>("servers");
  let query = $state("");

  let guilds = $state<GuildInfo[]>([]);
  let guildsLoading = $state(true);
  let guildsError = $state("");

  let dms = $state<DmInfo[]>([]);
  let dmsLoaded = $state(false);
  let dmsLoading = $state(false);
  let dmsError = $state("");

  let expanded = $state<Record<string, boolean>>({});
  let channels = $state<Record<string, GuildChannels>>({});
  let loadingGuild = $state<Record<string, boolean>>({});
  let guildError = $state<Record<string, string>>({});
  let protectError = $state("");

  const searching = $derived(query.trim() !== "");

  const visibleGuilds = $derived(
    guilds.filter((g) => {
      if (!searching) return true;
      if (matches(g.name, query)) return true;
      const c = channels[g.id];
      return !!c && filterGroups(c.groups, query).length > 0;
    }),
  );
  const visibleDms = $derived(dms.filter((d) => matches(d.name, query)));

  async function loadGuilds() {
    guildsLoading = true;
    guildsError = "";
    try {
      guilds = await listGuilds();
    } catch (e) {
      guildsError = String(e);
    } finally {
      guildsLoading = false;
    }
  }

  async function loadDms() {
    dmsLoading = true;
    dmsError = "";
    try {
      dms = await listDms();
      dmsLoaded = true;
    } catch (e) {
      dmsError = String(e);
    } finally {
      dmsLoading = false;
    }
  }

  async function ensureChannels(g: GuildInfo): Promise<GuildChannels | null> {
    if (channels[g.id]) return channels[g.id];
    loadingGuild[g.id] = true;
    guildError[g.id] = "";
    try {
      channels[g.id] = await listGuildChannels(g.id);
      return channels[g.id];
    } catch (e) {
      guildError[g.id] = String(e);
      return null;
    } finally {
      loadingGuild[g.id] = false;
    }
  }

  async function toggleGuild(g: GuildInfo) {
    expanded[g.id] = !expanded[g.id];
    if (expanded[g.id]) await ensureChannels(g);
  }

  function pick(c: ChannelInfo, where: string, guildId: string): SelectedChannel {
    return { id: c.id, name: c.name, where, guildId };
  }

  /** Channels of `gc` that may be selected (not protected, directly or via the server). */
  const selectable = (g: GuildInfo, gc: GuildChannels): ChannelInfo[] =>
    gc.groups.flatMap((grp) => grp.channels).filter((c) => !protectStore.isProtected(g.id, c.id));

  async function toggleWholeGuild(g: GuildInfo) {
    const gc = await ensureChannels(g);
    if (!gc) return;
    const list = selectable(g, gc);
    const ids = list.map((c) => c.id);
    if (ids.length > 0 && checkState(ids, selection.has) === "all") selection.removeMany(ids);
    else selection.addMany(list.map((c) => pick(c, g.name, g.id)));
  }

  function toggleGroup(g: GuildInfo, channelsInGroup: ChannelInfo[]) {
    const list = channelsInGroup.filter((c) => !protectStore.isProtected(g.id, c.id));
    const ids = list.map((c) => c.id);
    if (ids.length > 0 && checkState(ids, selection.has) === "all") selection.removeMany(ids);
    else selection.addMany(list.map((c) => pick(c, g.name, g.id)));
  }

  function toggleDm(d: DmInfo) {
    if (protectStore.isProtected(null, d.id)) return;
    selection.toggle({ id: d.id, name: d.name, where: "Direct messages", guildId: null });
  }

  // ------------------------------------------------------------ protect-list

  async function guarded(fn: () => Promise<void>) {
    protectError = "";
    try {
      await fn();
    } catch (e) {
      protectError = String(e);
    }
  }

  const dropFromSelection = (pred: (c: SelectedChannel) => boolean) =>
    selection.removeMany([...selection.items.values()].filter(pred).map((c) => c.id));

  const toggleProtectGuild = (g: GuildInfo) =>
    guarded(async () => {
      if (protectStore.has("guild", g.id)) {
        await protectStore.remove("guild", g.id);
      } else {
        await protectStore.add("guild", g.id, g.name);
        dropFromSelection((c) => c.guildId === g.id);
      }
    });

  const toggleProtectChannel = (id: string, name: string) =>
    guarded(async () => {
      if (protectStore.has("channel", id)) {
        await protectStore.remove("channel", id);
      } else {
        await protectStore.add("channel", id, name);
        dropFromSelection((c) => c.id === id);
      }
    });

  function setTab(t: Tab) {
    tab = t;
    if (t === "dms" && !dmsLoaded && !dmsLoading) loadDms();
  }

  function refresh() {
    channels = {};
    expanded = {};
    loadGuilds();
    if (dmsLoaded) loadDms();
  }

  onMount(() => {
    loadGuilds();
    protectStore.load().catch((e) => (protectError = String(e)));
  });
</script>

{#snippet check(state: "none" | "some" | "all", label: string, onchange: () => void, disabled = false)}
  <input
    type="checkbox"
    aria-label={label}
    checked={state === "all"}
    indeterminate={state === "some"}
    {disabled}
    {onchange}
  />
{/snippet}

{#snippet shield(isOn: boolean, locked: string | null, label: string, onclick: () => void)}
  <button
    class="shield"
    class:on={isOn}
    aria-pressed={isOn}
    aria-label={label}
    title={locked ?? (isOn ? "Protected: Concord won't scan or delete here. Click to unprotect." : "Protect: never scan or delete here")}
    disabled={locked !== null}
    {onclick}
  >
    {isOn ? "Protected" : "Protect"}
  </button>
{/snippet}

<section class="browse" aria-label="Browse servers and direct messages">
  <div class="toolbar">
    <div class="tabs" role="tablist">
      <button role="tab" aria-selected={tab === "servers"} class:active={tab === "servers"} onclick={() => setTab("servers")}>
        Servers {#if !guildsLoading}<span class="count">{guilds.length}</span>{/if}
      </button>
      <button role="tab" aria-selected={tab === "dms"} class:active={tab === "dms"} onclick={() => setTab("dms")}>
        Direct messages {#if dmsLoaded}<span class="count">{dms.length}</span>{/if}
      </button>
    </div>
    <input
      class="search"
      type="search"
      placeholder={tab === "servers" ? "Filter servers and channels…" : "Filter people…"}
      aria-label="Filter"
      bind:value={query}
    />
    <button class="ghost" onclick={refresh} title="Reload lists">Refresh</button>
  </div>

  {#if protectError}<p class="error pad" role="alert">{protectError}</p>{/if}

  <div class="list">
    {#if tab === "servers"}
      {#if guildsLoading}
        <p class="muted pad" role="status">Loading your servers…</p>
      {:else if guildsError}
        <p class="error pad" role="alert">{guildsError}</p>
        <button class="ghost pad" onclick={loadGuilds}>Try again</button>
      {:else if visibleGuilds.length === 0}
        <p class="muted pad">{searching ? "No servers or channels match." : "You aren't in any servers."}</p>
      {:else}
        {#each visibleGuilds as g (g.id)}
          {@const gc = channels[g.id]}
          {@const isOpen = expanded[g.id] || (searching && !!gc)}
          {@const ids = gc ? selectable(g, gc).map((c) => c.id) : []}
          {@const guildProtected = protectStore.has("guild", g.id)}
          <div class="guild" class:protected={guildProtected}>
            <div class="row">
              {@render check(
                checkState(ids, selection.has),
                `Select all channels in ${g.name}`,
                () => toggleWholeGuild(g),
                guildProtected,
              )}
              <button class="expander" aria-expanded={isOpen} onclick={() => toggleGuild(g)}>
                {#if guildIconUrl(g)}
                  <img class="icon" src={guildIconUrl(g)} alt="" loading="lazy" />
                {:else}
                  <span class="icon fallback" aria-hidden="true">{[...g.name][0]?.toUpperCase() ?? "?"}</span>
                {/if}
                <span class="name">{g.name}</span>
                <span class="chev" aria-hidden="true">{isOpen ? "▾" : "▸"}</span>
              </button>
              {@render shield(guildProtected, null, `Protect the server ${g.name}`, () => toggleProtectGuild(g))}
            </div>

            {#if isOpen}
              <div class="children">
                {#if loadingGuild[g.id]}
                  <p class="muted small" role="status">Loading channels…</p>
                {:else if guildError[g.id]}
                  <p class="error small" role="alert">{guildError[g.id]}</p>
                {:else if gc}
                  {#each filterGroups(gc.groups, query) as grp (grp.id ?? grp.name)}
                    {@const groupIds = grp.channels.filter((c) => !protectStore.isProtected(g.id, c.id)).map((c) => c.id)}
                    <div class="group">
                      <div class="row group-head">
                        {@render check(
                          checkState(groupIds, selection.has),
                          `Select all in ${grp.name}`,
                          () => toggleGroup(g, grp.channels),
                          groupIds.length === 0,
                        )}
                        <span class="group-name">{grp.name}</span>
                      </div>
                      {#each grp.channels as c (c.id)}
                        {@const reason = protectStore.reason(g.id, c.id)}
                        <div class="row-wrap" class:protected={reason !== null}>
                          <label class="row channel" title={reason ?? undefined}>
                            <input
                              type="checkbox"
                              aria-label={`Select ${c.name}`}
                              checked={selection.has(c.id)}
                              disabled={reason !== null}
                              onchange={() => selection.toggle(pick(c, g.name, g.id))}
                            />
                            <span class="kind">{KIND_LABEL[c.kind]}</span>
                            <span class="name">{c.name}</span>
                          </label>
                          {@render shield(
                            protectStore.has("channel", c.id),
                            guildProtected ? "The whole server is protected" : null,
                            `Protect the channel ${c.name}`,
                            () => toggleProtectChannel(c.id, c.name),
                          )}
                        </div>
                      {/each}
                    </div>
                  {/each}
                  {#if gc.threads_incomplete}
                    <p class="muted small">Couldn't list active threads for this server.</p>
                  {/if}
                {/if}
              </div>
            {/if}
          </div>
        {/each}
      {/if}
    {:else if dmsLoading}
      <p class="muted pad" role="status">Loading your direct messages…</p>
    {:else if dmsError}
      <p class="error pad" role="alert">{dmsError}</p>
      <button class="ghost pad" onclick={loadDms}>Try again</button>
    {:else if visibleDms.length === 0}
      <p class="muted pad">{searching ? "No one matches." : "No direct messages found."}</p>
    {:else}
      {#each visibleDms as d (d.id)}
        {@const dmProtected = protectStore.has("channel", d.id)}
        <div class="row-wrap" class:protected={dmProtected}>
          <label class="row channel dm">
            <input
              type="checkbox"
              aria-label={`Select ${d.name}`}
              checked={selection.has(d.id)}
              disabled={dmProtected}
              onchange={() => toggleDm(d)}
            />
            <span class="kind">{d.is_group ? "GRP" : "DM"}</span>
            <span class="name">{d.name}</span>
          </label>
          {@render shield(dmProtected, null, `Protect ${d.name}`, () => toggleProtectChannel(d.id, d.name))}
        </div>
      {/each}
      <p class="muted small pad">Listing DMs uses an undocumented Discord endpoint.</p>
    {/if}
  </div>

  <details class="protected-list">
    <summary>Protected ({protectStore.list.length})</summary>
    {#if protectStore.list.length === 0}
      <p class="muted small pad">
        Nothing is protected. Protect servers or channels that Concord must never scan or delete from.
      </p>
    {:else}
      <ul>
        {#each protectStore.list as p (p.kind + p.id)}
          <li>
            <span class="kind">{p.kind === "guild" ? "SRV" : "CH"}</span>
            <span class="name">{p.name}</span>
            <button class="ghost" onclick={() => guarded(() => protectStore.remove(p.kind, p.id))}>Remove</button>
          </li>
        {/each}
      </ul>
    {/if}
  </details>

  <footer class="bar" aria-live="polite">
    <span>
      {#if selection.count === 0}
        Nothing selected yet — tick channels or whole servers.
      {:else}
        <strong>{selection.count}</strong> {selection.count === 1 ? "channel" : "channels"} selected
      {/if}
    </span>
    {#if selection.count > 0}
      <button class="ghost" onclick={() => selection.clear()}>Clear</button>
    {/if}
  </footer>
</section>

<style>
  .browse { display: flex; flex-direction: column; border: 1px solid var(--line); border-radius: 12px; background: var(--card); overflow: hidden; }
  .toolbar { display: flex; gap: 10px; align-items: center; padding: 10px 12px; border-bottom: 1px solid var(--line); flex-wrap: wrap; }
  .tabs { display: flex; gap: 4px; }
  .tabs button { background: transparent; border: 1px solid transparent; padding: 6px 12px; }
  .tabs button.active { background: var(--bg); border-color: var(--line); }
  .count { color: var(--muted); font-size: 12px; margin-left: 4px; }
  .search { flex: 1; min-width: 160px; font: inherit; padding: 7px 10px; border-radius: 8px; border: 1px solid var(--line); background: var(--bg); color: var(--text); }
  .ghost { background: transparent; padding: 6px 12px; }
  .list { max-height: 52vh; min-height: 220px; overflow: auto; padding: 6px 0; }
  .pad { padding: 8px 14px; margin: 0; }
  .small { font-size: 13px; }
  .row { display: flex; align-items: center; gap: 10px; padding: 4px 14px; }
  .row input[type="checkbox"] { accent-color: var(--accent); width: 16px; height: 16px; flex: none; }
  .row-wrap { display: flex; align-items: center; padding-right: 14px; }
  .row-wrap .channel { flex: 1; min-width: 0; }
  .protected .channel, .guild.protected .expander { opacity: 0.55; }
  .expander { flex: 1; display: flex; align-items: center; gap: 10px; text-align: left; background: transparent; border: 0; padding: 6px 0; min-width: 0; }
  .icon { width: 28px; height: 28px; border-radius: 50%; object-fit: cover; flex: none; }
  .icon.fallback { display: grid; place-items: center; background: var(--bg); border: 1px solid var(--line); font-size: 13px; font-weight: 600; }
  .name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
  .chev { color: var(--muted); }
  .children { padding: 0 0 6px 28px; }
  .group-head { padding-top: 8px; }
  .group-name { color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0.04em; }
  .channel { cursor: pointer; border-radius: 6px; }
  .channel:hover { background: var(--bg); }
  .protected .channel { cursor: not-allowed; }
  .kind { color: var(--muted); font: 600 11px/1 ui-monospace, monospace; min-width: 28px; }
  .shield { font-size: 12px; padding: 3px 9px; background: transparent; color: var(--muted); flex: none; }
  .shield.on { color: #7ee2a8; border-color: #7ee2a8; }
  .shield:disabled { opacity: 0.4; cursor: not-allowed; }
  .protected-list { border-top: 1px solid var(--line); padding: 8px 14px; }
  .protected-list summary { cursor: pointer; color: var(--muted); }
  .protected-list ul { list-style: none; margin: 8px 0 0; padding: 0; display: flex; flex-direction: column; gap: 4px; }
  .protected-list li { display: flex; align-items: center; gap: 10px; }
  .bar { display: flex; align-items: center; justify-content: space-between; padding: 10px 14px; border-top: 1px solid var(--line); background: var(--bg); }
  .muted { color: var(--muted); }
  .error { color: var(--danger); }
  button { font: inherit; border-radius: 8px; border: 1px solid var(--line); color: var(--text); cursor: pointer; background: var(--card); }
  button:focus-visible, input:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
