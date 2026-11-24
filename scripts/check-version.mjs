// Release safety check: the version in the git tag must equal the version in
// package.json, src-tauri/tauri.conf.json and src-tauri/Cargo.toml, so an
// installer can never be published under the wrong version number.
//
//   node scripts/check-version.mjs v0.1.0     (the tag to release)
//   node scripts/check-version.mjs            (just check the three files agree)
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const TAG = /^v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)$/;

/** The `version = "x"` inside the [package] table of a Cargo.toml. */
export function cargoVersion(toml) {
  const pkg = toml.split(/^\[(?!package\])/m)[0]; // text up to the next table
  const m = /^\s*version\s*=\s*"([^"]+)"/m.exec(pkg.replace(/^[\s\S]*?\[package\]/, ""));
  return m ? m[1] : null;
}

/** Returns a list of problems (empty = fine). `tag` may be undefined. */
export function problems({ tag, versions }) {
  const out = [];
  const entries = Object.entries(versions);
  for (const [file, v] of entries) if (!v) out.push(`could not read a version from ${file}`);
  const distinct = new Set(entries.map(([, v]) => v).filter(Boolean));
  if (distinct.size > 1) out.push(`versions disagree: ${entries.map(([f, v]) => `${f}=${v}`).join(", ")}`);
  if (tag !== undefined) {
    const m = TAG.exec(tag);
    if (!m) out.push(`the tag "${tag}" is not of the form vMAJOR.MINOR.PATCH`);
    else if (distinct.size === 1 && !distinct.has(m[1])) out.push(`tag ${tag} does not match the version ${[...distinct][0]} in the project files`);
  }
  return out;
}

function main() {
  const read = (p) => readFileSync(new URL(`../${p}`, import.meta.url), "utf8");
  const versions = {
    "package.json": JSON.parse(read("package.json")).version,
    "src-tauri/tauri.conf.json": JSON.parse(read("src-tauri/tauri.conf.json")).version,
    "src-tauri/Cargo.toml": cargoVersion(read("src-tauri/Cargo.toml")),
  };
  const errs = problems({ tag: process.argv[2], versions });
  if (errs.length) {
    for (const e of errs) console.error("version check failed: " + e);
    process.exit(1);
  }
  console.log("versions OK: " + Object.values(versions)[0] + (process.argv[2] ? ` (tag ${process.argv[2]})` : ""));
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) main();
