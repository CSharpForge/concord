import { SvelteMap } from "svelte/reactivity";

/** A channel/DM the user picked for cleaning. `where` is the server name or "Direct messages". */
export interface SelectedChannel {
  id: string;
  name: string;
  where: string;
  /** The server the channel belongs to; `null` for DMs. */
  guildId: string | null;
}

class Selection {
  readonly items = new SvelteMap<string, SelectedChannel>();

  get count(): number {
    return this.items.size;
  }
  has = (id: string): boolean => this.items.has(id);

  toggle(c: SelectedChannel): void {
    if (this.items.has(c.id)) this.items.delete(c.id);
    else this.items.set(c.id, c);
  }
  addMany(cs: SelectedChannel[]): void {
    for (const c of cs) this.items.set(c.id, c);
  }
  removeMany(ids: string[]): void {
    for (const id of ids) this.items.delete(id);
  }
  clear(): void {
    this.items.clear();
  }
}

export const selection = new Selection();
