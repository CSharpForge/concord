import type { ChannelGroup } from "./discover";

export type CheckState = "none" | "some" | "all";

/** Case-insensitive substring match; an empty query matches everything. */
export function matches(name: string, query: string): boolean {
  const q = query.trim().toLowerCase();
  return q === "" || name.toLowerCase().includes(q);
}

/**
 * Keep groups/channels matching `query`. A group whose own name matches is
 * kept whole; otherwise only its matching channels are kept.
 */
export function filterGroups(groups: ChannelGroup[], query: string): ChannelGroup[] {
  if (query.trim() === "") return groups;
  return groups
    .map((g) => (matches(g.name, query) ? g : { ...g, channels: g.channels.filter((c) => matches(c.name, query)) }))
    .filter((g) => g.channels.length > 0);
}

/** Tri-state for a "select all" checkbox over `ids`. */
export function checkState(ids: string[], isSelected: (id: string) => boolean): CheckState {
  if (ids.length === 0) return "none";
  const n = ids.filter(isSelected).length;
  return n === 0 ? "none" : n === ids.length ? "all" : "some";
}
