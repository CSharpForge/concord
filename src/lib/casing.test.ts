import { describe, expect, it } from "vitest";

/**
 * Windows (Concord's only supported platform) ignores letter case in file
 * names, so `Jobs.svelte` and `jobs.svelte.ts` would collide there even though
 * they coexist fine on Linux/macOS. This test fails on any such collision, so
 * it is caught on every developer machine, not only in Windows CI.
 */
describe("source file names", () => {
  it("never differ only by letter case (once a trailing .ts/.js is dropped)", () => {
    const files = Object.keys(import.meta.glob("/src/**/*.{ts,js,svelte}"));
    expect(files.length).toBeGreaterThan(10); // the glob really found the sources

    const seen = new Map<string, string>();
    const clashes: string[] = [];
    for (const f of files) {
      const key = f.replace(/\.(ts|js)$/, "").toLowerCase();
      const other = seen.get(key);
      if (other && other !== f) clashes.push(`${other}  <->  ${f}`);
      seen.set(key, f);
    }
    expect(clashes).toEqual([]);
  });
});
