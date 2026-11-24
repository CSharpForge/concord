import type { Protected } from "./jobs";

/** Is this channel, or the server it belongs to (`null` for DMs), on the protect-list? */
export function isProtectedIn(list: Protected[], guildId: string | null | undefined, channelId: string): boolean {
  return list.some((p) => (p.kind === "channel" && p.id === channelId) || (p.kind === "guild" && !!guildId && p.id === guildId));
}

/** Why a channel is protected, for tooltips: the server, the channel, or `null` if it isn't. */
export function protectedReason(list: Protected[], guildId: string | null | undefined, channelId: string): string | null {
  const g = guildId ? list.find((p) => p.kind === "guild" && p.id === guildId) : undefined;
  if (g) return `The server "${g.name}" is protected`;
  const c = list.find((p) => p.kind === "channel" && p.id === channelId);
  return c ? `"${c.name}" is protected` : null;
}
