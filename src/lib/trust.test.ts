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

  it("has no inline style attributes or raw HTML (a strict CSP blocks inline styles; {@html} invites XSS)", () => {
    const bad: string[] = [];
    for (const [path, text] of appSources) {
      if (!path.endsWith(".svelte")) continue;
      // Ignore the <style> block itself; look at the markup.
      const markup = text.replace(/<style[\s\S]*?<\/style>/g, "");
      for (const line of markup.split("\n")) {
        if (/\sstyle\s*=\s*["'{]/.test(line) || /\{@html/.test(line)) bad.push(`${path}: ${line.trim()}`);
      }
    }
    expect(bad).toEqual([]);
  });

  it("the HTML template has no inline styles, scripts or event handlers (the CSP forbids them)", () => {
    const html = Object.values(import.meta.glob("/src/app.html", { query: "?raw", import: "default", eager: true })) as string[];
    expect(html).toHaveLength(1);
    const bad = html[0].split("\n").filter((l) => /\sstyle\s*=|\son[a-z]+\s*=|<script(?![^>]*\bsrc=)/i.test(l));
    expect(bad).toEqual([]);
  });

  it("the CSP's allowed inline style hash is exactly SvelteKit's announcer style (so a framework upgrade can't silently break it)", async () => {
    const generated = Object.values(
      import.meta.glob("/.svelte-kit/generated/root.svelte", { query: "?raw", import: "default", eager: true }),
    ) as string[];
    expect(generated, "run `svelte-kit sync` first").toHaveLength(1);
    const m = /id="svelte-announcer"[^>]*\sstyle="([^"]*)"/.exec(generated[0]);
    expect(m, "SvelteKit's announcer markup changed; update the CSP").not.toBeNull();
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(m![1]));
    const hash = "sha256-" + btoa(String.fromCharCode(...new Uint8Array(digest)));

    const conf = JSON.parse(
      (Object.values(import.meta.glob("/src-tauri/tauri.conf.json", { query: "?raw", import: "default", eager: true })) as string[])[0],
    );
    expect(conf.app.security.csp["style-src-attr"]).toBe(`'unsafe-hashes' '${hash}'`);
  });
});
