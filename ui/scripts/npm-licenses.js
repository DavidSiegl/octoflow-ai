// The licenses of the npm packages the desktop app's web view bundles, which MIT and the like require every copy of
// their code to come with: packaging/build-desktop.sh runs this after npm ci and ships what it writes beside the
// crates' notices (packaging/about.toml). Run as `node scripts/npm-licenses.js OUT` from ui/: writes the notices as
// HTML to OUT, or exits 1 naming every package that cannot be shipped.
//
// The packages are ui/package-lock.json's production tree, which is what ui/package.json's dependencies pull in, plus
// BUILD_CODE's, read from node_modules as npm ci laid it out, never from the network. That covers what Vite's bundle
// held on 2026-11-04, which the module comments of an unminified `vite build` list, and may name a package it shook
// out entirely.

import { existsSync, readdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/**
 * The dev packages that write code of their own into the bundle, so that it ships theirs too: Vite its module preload
 * polyfill, Rolldown, which Vite bundles with, its runtime (\0vite/... and \0rolldown/... modules in an unminified
 * build). A plugin that injects a runtime belongs here as well. Each is looked up at the top of node_modules. Since
 * such code may be copied from others, as Rolldown's runtime credits esbuild, each ships its THIRD-PARTY-LICENSE* and
 * NOTICE* files too.
 */
export const BUILD_CODE = ["vite", "rolldown"];

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
 *   package offered under several ("A OR B") is shipped under the first of them this list holds that one of its
 *   license files is named for (LICENSE-MIT); with none so named, its one license text is shipped under the whole
 *   expression when this list holds every license offered.
 * @returns {string} the page
 * @throws {Error} naming each package, one a line, that is under no accepted license, whose license is not an SPDX
 *   identifier or an OR of them, that has no one license text, whose one text, named for none of its licenses, may be
 *   that of one about.toml does not accept, or that is not installed although the lockfile
 *   does not mark it optional (an optional one npm ci skipped on this platform is not in the bundle); and, naming the
 *   file, when the lockfile or a package.json cannot be read
 */
export function npmNotices(ui, accepted) {
  const lock = readJson(join(ui, "package-lock.json"));
  /** @type {Map<string, {license: string, text: string, packages: {name: string, version: string}[]}>} by heading and text */
  const sections = new Map();
  const problems = [];
  for (const [path, entry] of Object.entries(lock.packages)) {
    // "" is ui/package.json itself, under the project's own license.
    if (path === "") continue;
    const buildCode = BUILD_CODE.some((name) => path === `node_modules/${name}`);
    if (entry.dev && !buildCode) continue;
    const dir = join(ui, path);
    if (!existsSync(join(dir, "package.json"))) {
      if (!entry.optional && !entry.devOptional) problems.push(`${path}: not installed`);
      continue;
    }
    const pkg = readJson(join(dir, "package.json"));
    const name = `${pkg.name} ${pkg.version}`;
    const offered = typeof pkg.license === "string" ? offeredLicenses(pkg.license) : undefined;
    const usable = offered ? accepted.filter((id) => offered.includes(id)) : [];
    if (usable.length === 0) {
      problems.push(`${name}: license ${JSON.stringify(pkg.license ?? pkg.licenses)} is none of about.toml's accepted`);
      continue;
    }
    const notice = licenseNotice(dir, offered, usable);
    if (notice.problem !== undefined) {
      problems.push(`${name}: ${notice.problem} in ${path}`);
      continue;
    }
    if (buildCode) notice.text = withThirdPartyNotices(dir, notice.text);
    const key = `${notice.license}\n${notice.text}`;
    const section = sections.get(key) ?? { ...notice, packages: [] };
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

// The notice of a package in dir offered under the licenses offered, of which usable are those about.toml accepts, in
// its order: a text from its LICENSE*, LICENCE* or COPYING* files and the license it is shipped under. The first of
// usable that exactly one file's name holds, as LICENSE-MIT does MIT and LICENSE-APACHE-2.0 Apache-2.0, gives that
// file under that license. Otherwise, when the files hold one text (LICENSE and LICENSE.md may hold it twice) and
// about.toml accepts every license offered, that text under all of them, since nothing says which it is. Otherwise,
// { problem }: why there is none, as a phrase about the expression: no file or files that differ, or one text of a
// package also offered under a license about.toml does not accept, which may be that license's text alone.
function licenseNotice(dir, offered, usable) {
  const files = noticeFiles(dir, /^(licen[cs]e|copying)/i);
  for (const license of usable) {
    const family = license.split("-")[0].toLowerCase();
    const named = files.filter((f) => f.name.toLowerCase().includes(family));
    if (named.length === 1) return { license, text: named[0].text };
  }
  const expression = offered.join(" OR ");
  if (files.length === 0 || files.some((f) => f.text !== files[0].text)) {
    return { problem: `no one license text for ${expression}` };
  }
  if (usable.length < offered.length) {
    return { problem: `one license text for ${expression}, not every license of which about.toml accepts` };
  }
  return { license: expression, text: files[0].text };
}

// text followed by each THIRD-PARTY-LICENSE*, THIRD-PARTY-LICENCE* or NOTICE* file in dir, by name, each headed by
// the file's name.
function withThirdPartyNotices(dir, text) {
  const extra = noticeFiles(dir, /^(third-party-licen[cs]e|notice)/i);
  return [text, ...extra.map((f) => `${f.name}:\n\n${f.text}`)].join("\n\n");
}

// The regular files in dir whose names match pattern, SPDX tag files (LICENSE.spdx) aside, by name, each with its
// trimmed text.
function noticeFiles(dir, pattern) {
  return readdirSync(dir, { withFileTypes: true })
    .filter((f) => f.isFile() && pattern.test(f.name) && !/\.spdx$/i.test(f.name))
    .map((f) => f.name)
    .sort()
    .map((name) => ({ name, text: readFileSync(join(dir, name), "utf8").trim() }));
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
