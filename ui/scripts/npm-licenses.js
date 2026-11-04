// The licenses of the npm packages the desktop app's web view bundles, which MIT and the like require every copy of
// their code to come with: packaging/build-desktop.sh runs this after npm ci and ships what it writes beside the
// crates' notices (packaging/about.toml). Run as `node scripts/npm-licenses.js OUT` from ui/: writes the notices as
// HTML to OUT, or exits 1 naming every package that cannot be shipped.
//
// The packages are ui/package-lock.json's production tree, which is what ui/package.json's dependencies pull in, plus
// BUILD_CODE's, read from node_modules as npm ci laid it out, never from the network. That covers what Vite's bundle
// holds after tree shaking, and may name a package it shook out entirely.

import { existsSync, readdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// Dev packages that write code of their own into the bundle, so it ships theirs too: Vite adds its module preload
// polyfill and its dynamic import helper. A plugin that injects a runtime belongs here as well.
const BUILD_CODE = ["vite"];

/**
 * The licenses about.toml's `accepted` array lists, in its order: the licenses a dependency may be used under.
 *
 * @param {string} aboutToml the text of packaging/about.toml
 * @returns {string[]} SPDX identifiers, first preferred
 * @throws {Error} when the text has no `accepted = [...]` array of strings
 */
export function acceptedLicenses(aboutToml) {
  const array = /^accepted = \[([^\]]*)\]/m.exec(aboutToml);
  const ids = array ? [...array[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]) : [];
  if (ids.length === 0) throw new Error("about.toml has no accepted = [...] list of licenses");
  return ids;
}

/**
 * The notices of the packages the app's bundle holds code of, installed under ui, as an HTML page: one section per
 * license and text, naming the packages that ship it.
 *
 * @param {string} ui the directory holding package-lock.json and the node_modules npm ci installed from it
 * @param {string[]} accepted the licenses a package may be under, first preferred, as acceptedLicenses gives them. A
 *   package offered under several ("A OR B") is shipped under the first of them this list holds.
 * @returns {string} the page
 * @throws {Error} naming each package, one a line, that is under no accepted license, whose license is not an SPDX
 *   identifier or an OR of them, that has no one license text for it, or that is not installed although the lockfile
 *   does not mark it optional (an optional one npm ci skipped on this platform is not in the bundle); and, naming the
 *   file, when the lockfile or a package.json cannot be read
 */
export function npmNotices(ui, accepted) {
  const lock = readJson(join(ui, "package-lock.json"));
  /** @type {Map<string, {license: string, text: string, packages: {name: string, version: string}[]}>} */
  const sections = new Map();
  const problems = [];
  for (const [path, entry] of Object.entries(lock.packages)) {
    // "" is ui/package.json itself, under the project's own license.
    if (path === "") continue;
    if (entry.dev && !BUILD_CODE.some((name) => path === `node_modules/${name}`)) continue;
    const dir = join(ui, path);
    if (!existsSync(join(dir, "package.json"))) {
      if (!entry.optional && !entry.devOptional) problems.push(`${path}: not installed`);
      continue;
    }
    const pkg = readJson(join(dir, "package.json"));
    const name = `${pkg.name} ${pkg.version}`;
    const offered = typeof pkg.license === "string" ? offeredLicenses(pkg.license) : undefined;
    const license = offered && accepted.find((id) => offered.includes(id));
    if (!license) {
      problems.push(`${name}: license ${JSON.stringify(pkg.license ?? pkg.licenses)} is none of about.toml's accepted`);
      continue;
    }
    const text = licenseText(dir, license, offered.length > 1);
    if (text === undefined) {
      problems.push(`${name}: no one license text for ${license} in ${path}`);
      continue;
    }
    const key = `${license}\n${text}`;
    const section = sections.get(key) ?? { license, text, packages: [] };
    section.packages.push({ name: pkg.name, version: pkg.version });
    sections.set(key, section);
  }
  if (problems.length > 0) throw new Error(`npm packages that cannot be shipped:\n${problems.join("\n")}`);
  return page([...sections.values()]);
}

function readJson(file) {
  try {
    return JSON.parse(readFileSync(file, "utf8"));
  } catch (err) {
    throw new Error(`reading ${file}: ${err instanceof Error ? err.message : err}`, { cause: err });
  }
}

// The licenses expression offers: an SPDX identifier, or several joined by OR, in parentheses or not; undefined for
// any other expression (AND, WITH).
function offeredLicenses(expression) {
  const offered = expression.replace(/^\((.*)\)$/, "$1").split(" OR ").map((id) => id.trim());
  return offered.every((id) => /^[A-Za-z0-9.+-]+$/.test(id)) ? offered : undefined;
}

// The text, trimmed, of license in dir's LICENSE*, LICENCE* or COPYING* files, SPDX tag files (LICENSE.spdx) aside.
// Several files with one text (LICENSE and LICENSE.md) count as one. Where the package offers several licenses, or
// the files differ, it is the one file whose name holds the license's name, as LICENSE-MIT does MIT and
// LICENSE-APACHE-2.0 Apache-2.0. Undefined when there is no such one text.
function licenseText(dir, license, severalOffered) {
  const files = readdirSync(dir, { withFileTypes: true })
    .filter((f) => f.isFile() && /^(licen[cs]e|copying)/i.test(f.name) && !/\.spdx$/i.test(f.name))
    .map((f) => ({ name: f.name, text: readFileSync(join(dir, f.name), "utf8").trim() }));
  if (!severalOffered && files.length > 0 && files.every((f) => f.text === files[0].text)) return files[0].text;
  const family = license.split("-")[0].toLowerCase();
  const named = files.filter((f) => f.name.toLowerCase().includes(family));
  return named.length === 1 ? named[0].text : undefined;
}

function escape(text) {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

// The page, in packaging/about.hbs's style so that the two notices files read alike.
function page(sections) {
  const body = sections
    .map(
      ({ text, license, packages }) => `    <section>
        <h3>${escape(license)}</h3>
        <p>Used by:</p>
        <ul class="used-by">
${packages
  .map(
    ({ name, version }) =>
      `            <li><a href="https://www.npmjs.com/package/${escape(name)}">${escape(name)} ${escape(version)}</a></li>`,
  )
  .join("\n")}
        </ul>
        <pre>${escape(text)}</pre>
    </section>`,
    )
    .join("\n");
  return `<!DOCTYPE html>
<!-- Written by ui/scripts/npm-licenses.js. -->
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Octoflow AI: the desktop app's npm packages' licenses</title>
    <style>
        :root { color-scheme: light dark; }
        body { font-family: system-ui, sans-serif; max-width: 60rem; margin: 2rem auto; padding: 0 1rem; line-height: 1.5; }
        pre { white-space: pre-wrap; padding: 1rem; border: 1px solid #8884; border-radius: 4px; font-size: 0.85rem; }
        .used-by { columns: 16rem; padding-left: 1.2rem; }
    </style>
</head>
<body>
    <h1>Third-party licenses: npm packages</h1>
    <p>
        The Octoflow AI desktop app's window contains code of the following npm packages, under the licenses below. The Rust
        crates it contains are in THIRD-PARTY-LICENSES.html. Octoflow AI itself is licensed under MIT OR Apache-2.0.
    </p>
${body}
</body>
</html>
`;
}

// Run directly, not imported by the tests. argv[1] is the path as invoked, which may pass through a symbolic link.
if (process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const out = process.argv[2];
  if (!out || process.argv.length > 3) {
    console.error(`usage: node ${basename(process.argv[1])} OUT`);
    process.exit(2);
  }
  const ui = dirname(dirname(fileURLToPath(import.meta.url)));
  try {
    const accepted = acceptedLicenses(readFileSync(join(ui, "..", "packaging", "about.toml"), "utf8"));
    writeFileSync(out, npmNotices(ui, accepted));
  } catch (err) {
    console.error(`npm-licenses.js: ${err instanceof Error ? err.message : err}`);
    process.exit(1);
  }
}
