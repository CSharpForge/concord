import { describe, expect, it } from "vitest";
import type { ChannelGroup } from "./discover";
import { checkState, filterGroups, matches } from "./filter";

const groups: ChannelGroup[] = [
  {
    id: "1",
    name: "Text",
    channels: [
      { id: "a", name: "general", kind: "text" },
      { id: "b", name: "memes", kind: "text" },
    ],
  },
  { id: "2", name: "Voice", channels: [{ id: "c", name: "lounge", kind: "voice" }] },
];

describe("matches", () => {
  it("is case-insensitive and trims", () => {
    expect(matches("General", "  gEN ")).toBe(true);
    expect(matches("General", "xyz")).toBe(false);
  });
  it("matches everything for an empty query", () => {
    expect(matches("anything", "")).toBe(true);
    expect(matches("anything", "   ")).toBe(true);
  });
});

describe("filterGroups", () => {
  it("returns the same groups for an empty query", () => {
    expect(filterGroups(groups, "")).toBe(groups);
  });
  it("keeps only matching channels", () => {
    const r = filterGroups(groups, "meme");
    expect(r).toHaveLength(1);
    expect(r[0].channels.map((c) => c.id)).toEqual(["b"]);
  });
  it("keeps a whole group when its name matches", () => {
    const r = filterGroups(groups, "voice");
    expect(r[0].channels.map((c) => c.id)).toEqual(["c"]);
    expect(filterGroups(groups, "text")[0].channels).toHaveLength(2);
  });
  it("drops groups with no matches and does not mutate input", () => {
    expect(filterGroups(groups, "zzz")).toEqual([]);
    expect(groups[0].channels).toHaveLength(2);
  });
});

describe("checkState", () => {
  const sel = new Set(["a"]);
  it("reports none / some / all", () => {
    expect(checkState(["a", "b"], (id) => sel.has(id))).toBe("some");
    expect(checkState(["b"], (id) => sel.has(id))).toBe("none");
    expect(checkState(["a"], (id) => sel.has(id))).toBe("all");
  });
  it("treats an empty list as none", () => {
    expect(checkState([], () => true)).toBe("none");
  });
});
