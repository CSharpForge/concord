import { describe, expect, it } from "vitest";

/**
 * Promises made in docs/TOKEN-HANDLING.md about the web page (UI), enforced as tests.
 * Test files and comment lines are ignored.
 */
const sources = import.meta.glob("/src/**/*.{ts,svelte}", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

const appSources = Object.entries(sources).filter(([path]) => !path.endsWith(".test.ts"));

/** Lines of code: drop `//` comment lines (URLs in comments are not requests). */
const codeLines = (text: string) => text.split("\n").filter((l) => !l.trim().startsWith("//"));

describe("the UI's network use", () => {
  it("finds the app's sources", () => {
    expect(appSources.length).toBeGreaterThan(8);
  });

  it("only references Discord's image CDN", () => {
    const ALLOWED = new Set(["cdn.discordapp.com"]);
    const bad: string[] = [];
    for (const [path, text] of appSources) {
      for (const line of codeLines(text)) {
        for (const m of line.matchAll(/https?:\/\/([A-Za-z0-9.-]+)/g)) {
          if (!ALLOWED.has(m[1])) bad.push(`${path}: ${m[0]}`);
        }
      }
    }
    expect(bad).toEqual([]);
  });

  it("never calls fetch or opens sockets itself (all network goes through the Rust backend)", () => {
    const bad: string[] = [];
    for (const [path, text] of appSources) {
      for (const line of codeLines(text)) {
        if (/\bfetch\s*\(|new\s+(WebSocket|XMLHttpRequest|EventSource)\b|navigator\.sendBeacon/.test(line)) bad.push(`${path}: ${line.trim()}`);
      }
    }
    expect(bad).toEqual([]);
  });

  it("uses no Tauri plugin APIs (their permissions are not granted)", () => {
    const bad = appSources.filter(([, text]) => /@tauri-apps\/plugin-/.test(text)).map(([p]) => p);
    expect(bad).toEqual([]);
  });

  it("never touches storage with anything token-like", () => {
    const bad: string[] = [];
    for (const [path, text] of appSources) {
      for (const line of codeLines(text)) {
        if (/(localStorage|sessionStorage|document\.cookie)/.test(line)) bad.push(`${path}: ${line.trim()}`);
      }
    }
    expect(bad).toEqual([]);
  });
});
