import { describe, expect, it } from "vitest";
import type { Protected } from "./jobs";
import { isProtectedIn, protectedReason } from "./protect";

const list: Protected[] = [
  { kind: "channel", id: "c1", name: "#general" },
  { kind: "guild", id: "g1", name: "Family" },
];

describe("isProtectedIn", () => {
  it("protects a listed channel, in any server", () => {
    expect(isProtectedIn(list, "g9", "c1")).toBe(true);
    expect(isProtectedIn(list, null, "c1")).toBe(true); // e.g. a DM channel id
  });
  it("protects every channel of a listed server", () => {
    expect(isProtectedIn(list, "g1", "c-anything")).toBe(true);
  });
  it("leaves everything else alone", () => {
    expect(isProtectedIn(list, "g9", "c9")).toBe(false);
    expect(isProtectedIn([], "g1", "c1")).toBe(false);
  });
  it("a DM (no server) is never covered by a server entry", () => {
    expect(isProtectedIn([{ kind: "guild", id: "", name: "x" }], null, "c9")).toBe(false);
    expect(isProtectedIn(list, undefined, "c9")).toBe(false);
  });
  it("does not confuse a channel id with a server id", () => {
    expect(isProtectedIn([{ kind: "channel", id: "7", name: "c" }], "7", "8")).toBe(false);
    expect(isProtectedIn([{ kind: "guild", id: "7", name: "g" }], null, "7")).toBe(false);
  });
});

describe("protectedReason", () => {
  it("explains whether the server or the channel is protected, preferring the server", () => {
    expect(protectedReason(list, "g1", "c1")).toBe('The server "Family" is protected');
    expect(protectedReason(list, "g9", "c1")).toBe('"#general" is protected');
    expect(protectedReason(list, "g9", "c9")).toBeNull();
  });
});
