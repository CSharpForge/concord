import { describe, expect, it } from "vitest";
import { displayName, initial } from "./user";

describe("displayName", () => {
  it("prefers the global display name", () => {
    expect(displayName({ username: "alice", global_name: "Alice W" })).toBe("Alice W");
  });
  it("falls back to username when the display name is missing or blank", () => {
    expect(displayName({ username: "alice", global_name: null })).toBe("alice");
    expect(displayName({ username: "alice" })).toBe("alice");
    expect(displayName({ username: "alice", global_name: "   " })).toBe("alice");
  });
});

describe("initial", () => {
  it("uppercases the first letter", () => {
    expect(initial({ username: "bob" })).toBe("B");
  });
  it("handles non-BMP characters without splitting them", () => {
    expect(initial({ username: "x", global_name: "😀 fun" })).toBe("😀");
  });
});
