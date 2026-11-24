import { invoke } from "@tauri-apps/api/core";

export interface GuildInfo {
  id: string;
  name: string;
  icon: string | null;
}

export type ChannelKind = "text" | "announcement" | "voice" | "stage" | "forum" | "media" | "thread";

export interface ChannelInfo {
  id: string;
  name: string;
  kind: ChannelKind;
}

export interface ChannelGroup {
  id: string | null;
  name: string;
  channels: ChannelInfo[];
}

export interface GuildChannels {
  groups: ChannelGroup[];
  threads_incomplete: boolean;
}

export interface DmInfo {
  id: string;
  name: string;
  is_group: boolean;
}

export const listGuilds = () => invoke<GuildInfo[]>("list_guilds");
export const listDms = () => invoke<DmInfo[]>("list_dms");
export const listGuildChannels = (guildId: string) =>
  invoke<GuildChannels>("list_guild_channels", { guildId });

export function guildIconUrl(g: Pick<GuildInfo, "id" | "icon">, size = 64): string | null {
  return g.icon ? `https://cdn.discordapp.com/icons/${g.id}/${g.icon}.png?size=${size}` : null;
}

export const KIND_LABEL: Record<ChannelKind, string> = {
  text: "#",
  announcement: "ANN",
  voice: "VC",
  stage: "STG",
  forum: "FRM",
  media: "MED",
  thread: "THR",
};
