// The licenses of the npm packages the desktop app's web view bundles, which MIT and the like require every copy of
// their code to come with: packaging/build-desktop.sh runs this after npm ci and ships what it writes beside the
// crates' notices (packaging/about.toml). Run as `node scripts/npm-licenses.js OUT` from ui/: writes the notices as
// HTML to OUT, or exits 1 naming every package that cannot be shipped.
//
// The packages are ui/package-lock.json's production tree: every installed entry not marked dev, which is what
// ui/package.json's dependencies pull in, read from node_modules as npm ci laid it out, never from the network. That is
// a superset of what Vite's bundle holds after tree shaking, so nothing the bundle holds is left out.

import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

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
 * The notices of the production packages installed under ui, as an HTML page: one section per distinct license text,
 * naming the packages that ship it.
 *
 * @param {string} ui the directory holding package-lock.json and the node_modules npm ci installed from it
 * @param {string[]} accepted the licenses a package may be under, first preferred, as acceptedLicenses gives them. A
 *   package offered under several ("A OR B") is shipped under the first of them this list holds.
 * @returns {string} the page
 * @throws {Error} naming each package, one a line, that is under no accepted license, whose license is not an SPDX
 *   identifier or an OR of them, or that has no one license file for it; and when the lockfile or a package.json
 *   cannot be read
 */
export function npmNotices(ui, accepted) {
  const lock = JSON.parse(readFileSync(join(ui, "package-lock.json"), "utf8"));
  /** @type {Map<string, {license: string, packages: {name: string, version: string}[]}>} keyed by license text */
  const texts = new Map();
  const problems = [];
  for (const [path, entry] of Object.entries(lock.packages)) {
    // "" is ui/package.json itself, under the project's own license.
    if (path === "" || entry.dev) continue;
    const dir = join(ui, path);
    const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
    const name = `${pkg.name} ${pkg.version}`;
    const license = typeof pkg.license === "string" ? chosen(pkg.license, accepted) : undefined;
    if (!license) {
      problems.push(`${name}: license ${JSON.stringify(pkg.license ?? pkg.licenses)} is none of about.toml's accepted`);
      continue;
    }
    const file = licenseFile(dir, license);
    if (!file) {
      problems.push(`${name}: no one license file for ${license} in ${path}`);
      continue;
    }
    const text = readFileSync(join(dir, file), "utf8").trim();
    const shipped = texts.get(text) ?? { license, packages: [] };
    shipped.packages.push({ name: pkg.name, version: pkg.version });
    texts.set(text, shipped);
  }
  if (problems.length > 0) throw new Error(`npm packages that cannot be shipped:\n${problems.join("\n")}`);
  return page([...texts].map(([text, shipped]) => ({ text, ...shipped })));
}

// The first of accepted that expression offers: an SPDX identifier, or several joined by OR, in parentheses or not;
// undefined for any other expression (AND, WITH) or when it offers none of them.
function chosen(expression, accepted) {
  const offered = expression.replace(/^\((.*)\)$/, "$1").split(" OR ").map((id) => id.trim());
  if (offered.some((id) => !/^[A-Za-z0-9.+-]+$/.test(id))) return undefined;
  return accepted.find((id) => offered.includes(id));
}

// The name of the file in dir that holds license's text: its only LICENSE*, LICENCE* or COPYING* file other than an
// SPDX tag file (LICENSE.spdx), or, where there are several, the only one whose name holds the license's name, as
// LICENSE-MIT does MIT and LICENSE-APACHE-2.0 Apache-2.0; undefined when there is no such one file.
function licenseFile(dir, license) {
  const files = readdirSync(dir).filter((f) => /^(licen[cs]e|copying)/i.test(f) && !/\.spdx$/i.test(f));
  if (files.length === 1) return files[0];
  const family = license.split("-")[0].toLowerCase();
  const named = files.filter((f) => f.toLowerCase().includes(family));
  return named.length === 1 ? named[0] : undefined;
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
        The Octoflow AI desktop app's window contains the following npm packages, under the licenses below. The Rust
        crates it contains are in THIRD-PARTY-LICENSES.html. Octoflow AI itself is licensed under MIT OR Apache-2.0.
    </p>
${body}
</body>
</html>
`;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
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
