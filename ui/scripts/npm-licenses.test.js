import { mkdtempSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { acceptedLicenses, npmNotices } from "./npm-licenses.js";

const ui = dirname(dirname(fileURLToPath(import.meta.url)));
const accepted = acceptedLicenses(readFileSync(join(ui, "..", "packaging", "about.toml"), "utf8"));

function escaped(text) {
  return text.trim().replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

describe("the notices of the app's own npm packages", () => {
  const lock = JSON.parse(readFileSync(join(ui, "package-lock.json"), "utf8"));
  const entries = Object.entries(lock.packages).filter(([path]) => path !== "");
  const html = npmNotices(ui, accepted);
  // Whether the notices name the package the lockfile installs at path, by the name and version it gives it.
  const listed = (path) => html.includes(`>${path.split("node_modules/").pop()} ${lock.packages[path].version}</a>`);
  // The text of the package's one LICENSE or license file.
  const licenseText = (path) => {
    const [file] = readdirSync(join(ui, path)).filter((f) => /^license$/i.test(f));
    return readFileSync(join(ui, path, file), "utf8");
  };

  it("name every package of the production tree", () => {
    const missing = entries.filter(([path, e]) => !e.dev && !listed(path)).map(([path]) => path);
    expect(missing).toEqual([]);
    expect(listed("node_modules/react")).toBe(true);
    expect(listed("node_modules/react-markdown")).toBe(true);
  });

  it("name no dev-only package", () => {
    const prod = new Set(entries.filter(([, e]) => !e.dev).map(([path]) => path.split("node_modules/").pop()));
    const dev = entries.filter(([path, e]) => e.dev && !prod.has(path.split("node_modules/").pop()));
    expect(dev.length).toBeGreaterThan(0);
    expect(dev.filter(([path]) => listed(path)).map(([path]) => path)).toEqual([]);
  });

  it("give each package its license text", () => {
    for (const path of ["node_modules/react", "node_modules/react-markdown", "node_modules/remark-gfm"]) {
      expect(html).toContain(escaped(licenseText(path)));
    }
    // Apache-2.0 OR MIT: about.toml lists MIT first.
    expect(html).toContain(escaped(readFileSync(join(ui, "node_modules/@tauri-apps/api/LICENSE-MIT"), "utf8")));
  });
});

describe("npmNotices on a made-up tree", () => {
  let root;
  afterEach(() => rmSync(root, { recursive: true, force: true }));

  // A ui directory whose lockfile lists each of packages, installed with its license and files.
  function tree(packages) {
    root = mkdtempSync(join(tmpdir(), "npm-licenses-"));
    const lock = { lockfileVersion: 3, packages: { "": { name: "app" } } };
    for (const { name, license, files = {}, dev = false } of packages) {
      const dir = join(root, "node_modules", name);
      mkdirSync(dir, { recursive: true });
      writeFileSync(join(dir, "package.json"), JSON.stringify({ name, version: "1.0.0", license }));
      for (const [file, text] of Object.entries(files)) writeFileSync(join(dir, file), text);
      lock.packages[`node_modules/${name}`] = { version: "1.0.0", ...(dev ? { dev } : {}) };
    }
    writeFileSync(join(root, "package-lock.json"), JSON.stringify(lock));
    return root;
  }

  it("ships a package under a license about.toml does not accept nowhere, naming it", () => {
    const ui = tree([
      { name: "copyleft", license: "GPL-3.0-only", files: { LICENSE: "GPL" } },
      { name: "fine", license: "MIT", files: { LICENSE: "MIT" } },
    ]);
    expect(() => npmNotices(ui, accepted)).toThrow(/^npm packages that cannot be shipped:\ncopyleft 1\.0\.0: /);
  });

  it("names a package that has no license file", () => {
    const ui = tree([{ name: "bare", license: "MIT" }]);
    expect(() => npmNotices(ui, accepted)).toThrow(/bare 1\.0\.0: no one license file for MIT/);
  });

  it("names every package that cannot be shipped, not only the first", () => {
    const ui = tree([
      { name: "one", license: "GPL-3.0-only", files: { LICENSE: "x" } },
      { name: "two", license: "MIT AND GPL-3.0-only", files: { LICENSE: "x" } },
    ]);
    expect(() => npmNotices(ui, accepted)).toThrow(/one 1\.0\.0: .*\ntwo 1\.0\.0: /);
  });

  it("takes the file of the first accepted license of an OR", () => {
    const ui = tree([
      {
        name: "dual",
        license: "(Apache-2.0 OR MIT)",
        files: { "LICENSE-APACHE": "apache text", "LICENSE-MIT": "mit text", "LICENSE.spdx": "tag" },
      },
    ]);
    const html = npmNotices(ui, accepted);
    expect(html).toContain("<h3>MIT</h3>");
    expect(html).toContain("<pre>mit text</pre>");
    expect(html).not.toContain("apache text");
  });

  it("lists the packages sharing a license text under one section, and leaves dev packages out", () => {
    const ui = tree([
      { name: "a", license: "ISC", files: { LICENSE: "same <text>" } },
      { name: "b", license: "ISC", files: { "LICENSE.md": "same <text>" } },
      { name: "tool", license: "ISC", files: { LICENSE: "same <text>" }, dev: true },
    ]);
    const html = npmNotices(ui, accepted);
    expect(html.match(/<pre>/g)).toHaveLength(1);
    expect(html).toContain("<pre>same &lt;text&gt;</pre>");
    expect(html).toContain(">a 1.0.0</a>");
    expect(html).toContain(">b 1.0.0</a>");
    expect(html).not.toContain(">tool 1.0.0</a>");
  });
});

describe("acceptedLicenses", () => {
  it("reads about.toml's list in its order", () => {
    expect(accepted.slice(0, 2)).toEqual(["MIT", "Apache-2.0"]);
  });

  it("refuses a text without the list", () => {
    expect(() => acceptedLicenses("targets = []\n")).toThrow(/no accepted/);
  });
});
