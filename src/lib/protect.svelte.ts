import { addProtected, listProtected, removeProtected, type Protected } from "./jobs";
import { isProtectedIn, protectedReason } from "./protect";

/** The protect-list: channels and servers Concord refuses to scan or delete. */
class ProtectStore {
  list = $state<Protected[]>([]);
  loaded = $state(false);

  async load(): Promise<void> {
    this.list = await listProtected();
    this.loaded = true;
  }

  isProtected = (guildId: string | null | undefined, channelId: string): boolean =>
    isProtectedIn(this.list, guildId, channelId);

  reason = (guildId: string | null | undefined, channelId: string): string | null =>
    protectedReason(this.list, guildId, channelId);

  has = (kind: Protected["kind"], id: string): boolean => this.list.some((p) => p.kind === kind && p.id === id);

  async add(kind: Protected["kind"], id: string, name: string): Promise<void> {
    await addProtected(kind, id, name);
    await this.load();
  }

  async remove(kind: Protected["kind"], id: string): Promise<void> {
    await removeProtected(kind, id);
    await this.load();
  }
}

export const protectStore = new ProtectStore();
