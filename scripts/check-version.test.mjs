import { describe, expect, it } from "vitest";
import { cargoVersion, problems } from "./check-version.mjs";

const ok = { "package.json": "0.2.0", "src-tauri/tauri.conf.json": "0.2.0", "src-tauri/Cargo.toml": "0.2.0" };

describe("cargoVersion", () => {
  it("reads the [package] version, not a dependency's", () => {
    const toml = `[package]\nname = "concord"\nversion = "0.2.0"\nedition = "2024"\n\n[dependencies]\ntauri = { version = "2" }\nreqwest = { version = "0.13" }\n[dev-dependencies]\ntokio = { version = "1.54.1" }\n`;
    expect(cargoVersion(toml)).toBe("0.2.0");
  });
  it("returns null when there is none", () => expect(cargoVersion("[package]\nname = \"x\"\n")).toBeNull());
});

describe("problems", () => {
  it("accepts matching files and a matching tag", () => {
    expect(problems({ tag: "v0.2.0", versions: ok })).toEqual([]);
    expect(problems({ tag: undefined, versions: ok })).toEqual([]);
  });
  it("accepts pre-release tags when the files carry the same version", () => {
    const v = { ...ok, "package.json": "0.3.0-rc.1", "src-tauri/tauri.conf.json": "0.3.0-rc.1", "src-tauri/Cargo.toml": "0.3.0-rc.1" };
    expect(problems({ tag: "v0.3.0-rc.1", versions: v })).toEqual([]);
  });
  it("rejects a tag that does not match", () => {
    expect(problems({ tag: "v0.2.1", versions: ok })[0]).toMatch(/does not match/);
  });
  it("rejects files that disagree", () => {
    const bad = { ...ok, "src-tauri/Cargo.toml": "0.1.0" };
    expect(problems({ tag: "v0.2.0", versions: bad })[0]).toMatch(/disagree/);
  });
  it("rejects malformed tags", () => {
    for (const t of ["0.2.0", "v0.2", "v1.2.3.4", "release-1", "v0.2.0 ", "vx.y.z"]) expect(problems({ tag: t, versions: ok }).length, t).toBeGreaterThan(0);
  });
  it("rejects a missing version", () => {
    expect(problems({ tag: "v0.2.0", versions: { ...ok, "package.json": undefined } })[0]).toMatch(/could not read/);
  });
});
