import { mkdtempSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { acceptedLicenses, BUILD_CODE, npmNotices } from "./npm-licenses.js";

const ui = dirname(dirname(fileURLToPath(import.meta.url)));
const accepted = acceptedLicenses(readFileSync(join(ui, "..", "packaging", "about.toml"), "utf8"));

// The notices page as its reader sees it: each section's heading, its packages as "name version", and its text.
function sections(html) {
  const doc = new DOMParser().parseFromString(html, "text/html");
  return [...doc.querySelectorAll("section")].map((s) => ({
    license: s.querySelector("h3")?.textContent,
    packages: [...s.querySelectorAll("li")].map((li) => li.textContent),
    text: s.querySelector("pre")?.textContent,
  }));
}

describe("the notices of the app's own npm packages", () => {
  const lock = JSON.parse(readFileSync(join(ui, "package-lock.json"), "utf8"));
  const entries = Object.entries(lock.packages).filter(([path]) => path !== "");
  const shown = sections(npmNotices(ui, accepted));
  const names = new Set(shown.flatMap((s) => s.packages));
  // The name and version the lockfile installs at path, as the notices list a package.
  const named = (path) => `${path.split("node_modules/").pop()} ${lock.packages[path].version}`;
  // The section listing the package at path.
  const sectionOf = (path) => shown.find((s) => s.packages.includes(named(path)));
  // The trimmed text of the package's LICENSE file, whatever its case and extension.
  const licenseText = (path) => {
    const [file] = readdirSync(join(ui, path)).filter((f) => /^license(\.md|\.txt)?$/i.test(f));
    expect(file, `${path} has a LICENSE file`).toBeDefined();
    return readFileSync(join(ui, path, file), "utf8").trim();
  };

  const buildCode = BUILD_CODE.map((name) => `node_modules/${name}`);

  it("name every package of the production tree, and those whose code the build adds", () => {
    const missing = entries.filter(([path, e]) => !e.dev && !names.has(named(path))).map(([path]) => path);
    expect(missing).toEqual([]);
    for (const path of ["node_modules/react", "node_modules/react-markdown", ...buildCode]) {
      expect(names).toContain(named(path));
    }
  });

  it("name no other dev package", () => {
    const kept = entries.filter(([path, e]) => !e.dev || buildCode.includes(path));
    const prod = new Set(kept.map(([path]) => named(path)));
    const dev = entries.map(([path]) => named(path)).filter((n) => !prod.has(n));
    expect(dev.length).toBeGreaterThan(0);
    expect(dev.filter((n) => names.has(n))).toEqual([]);
  });

  it("give each package its license text", () => {
    for (const path of ["node_modules/react", "node_modules/react-markdown", "node_modules/remark-gfm"]) {
      expect(sectionOf(path)?.text).toBe(licenseText(path));
    }
    // Apache-2.0 OR MIT: about.toml lists MIT first.
    const tauri = "node_modules/@tauri-apps/api";
    expect(sectionOf(tauri)?.license).toBe("MIT");
    expect(sectionOf(tauri)?.text).toBe(readFileSync(join(ui, tauri, "LICENSE-MIT"), "utf8").trim());
  });
});

describe("npmNotices on a made-up tree", () => {
  let root;
  afterEach(() => rmSync(root, { recursive: true, force: true }));

  // A ui directory whose lockfile lists each of packages with its lock flags, installed with its license and files
  // unless installed is false. A file whose text is null is made a directory.
  function tree(packages) {
    root = mkdtempSync(join(tmpdir(), "npm-licenses-"));
    const lock = { lockfileVersion: 3, packages: { "": { name: "app" } } };
    for (const { name, license, files = {}, flags = {}, installed = true } of packages) {
      lock.packages[`node_modules/${name}`] = { version: "1.0.0", ...flags };
      if (!installed) continue;
      const dir = join(root, "node_modules", name);
      mkdirSync(dir, { recursive: true });
      writeFileSync(join(dir, "package.json"), JSON.stringify({ name, version: "1.0.0", license }));
      for (const [file, text] of Object.entries(files)) {
        if (text === null) mkdirSync(join(dir, file));
        else writeFileSync(join(dir, file), text);
      }
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
    const ui = tree([{ name: "bare", license: "MIT", files: { licenses: null } }]);
    expect(() => npmNotices(ui, accepted)).toThrow(/bare 1\.0\.0: no one license text for MIT/);
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
    expect(sections(npmNotices(ui, accepted))).toEqual([{ license: "MIT", packages: ["dual 1.0.0"], text: "mit text" }]);
  });

  it("takes the file named for a later accepted license of an OR when none is named for the first", () => {
    const ui = tree([
      { name: "apache-file", license: "Apache-2.0 OR MIT", files: { "LICENSE-APACHE": "apache text" } },
    ]);
    expect(sections(npmNotices(ui, accepted))).toEqual([
      { license: "Apache-2.0", packages: ["apache-file 1.0.0"], text: "apache text" },
    ]);
  });

  it("gives a package offered under accepted licenses only, whose file names none, the whole expression", () => {
    const ui = tree([{ name: "one-file", license: "MIT OR Apache-2.0", files: { LICENSE: "both texts" } }]);
    expect(sections(npmNotices(ui, accepted))).toEqual([
      { license: "MIT OR Apache-2.0", packages: ["one-file 1.0.0"], text: "both texts" },
    ]);
  });

  it("names a package also offered under a license about.toml does not accept whose file names none of them", () => {
    // The one text may be GPL's alone, so it cannot stand for the MIT notice.
    const ui = tree([{ name: "maybe-gpl", license: "MIT OR GPL-3.0-only", files: { COPYING: "gpl text" } }]);
    const refused = /maybe-gpl 1\.0\.0: no one license text for MIT OR GPL-3\.0-only/;
    expect(() => npmNotices(ui, accepted)).toThrow(refused);
  });

  it("names a package whose license files differ and none is named for its license", () => {
    const ui = tree([{ name: "two", license: "MIT", files: { LICENSE: "one", "LICENSE-OTHER": "two" } }]);
    expect(() => npmNotices(ui, accepted)).toThrow(/two 1\.0\.0: no one license text for MIT/);
  });

  it("takes one text from several files that hold it", () => {
    const ui = tree([{ name: "twice", license: "MIT", files: { LICENSE: "same\n", "LICENSE.md": "same" } }]);
    expect(sections(npmNotices(ui, accepted))).toEqual([{ license: "MIT", packages: ["twice 1.0.0"], text: "same" }]);
  });

  it("lists the packages sharing a license and its text under one section, and leaves dev packages out", () => {
    const ui = tree([
      { name: "a", license: "ISC", files: { LICENSE: "same <text>" } },
      { name: "b", license: "ISC", files: { "LICENSE.md": "same <text>" } },
      { name: "c", license: "MIT", files: { LICENSE: "same <text>" } },
      { name: "tool", license: "ISC", files: { LICENSE: "same <text>" }, flags: { dev: true } },
    ]);
    expect(sections(npmNotices(ui, accepted))).toEqual([
      { license: "ISC", packages: ["a 1.0.0", "b 1.0.0"], text: "same <text>" },
      { license: "MIT", packages: ["c 1.0.0"], text: "same <text>" },
    ]);
  });

  it("skips an optional package npm ci did not install, and names a missing one that is not optional", () => {
    const skipped = tree([
      { name: "a", license: "MIT", files: { LICENSE: "a" } },
      { name: "other-os", license: "MIT", flags: { optional: true }, installed: false },
      { name: "other-os-too", license: "MIT", flags: { devOptional: true }, installed: false },
    ]);
    expect(sections(npmNotices(skipped, accepted)).flatMap((s) => s.packages)).toEqual(["a 1.0.0"]);
    rmSync(root, { recursive: true, force: true });
    const missing = tree([{ name: "gone", license: "MIT", installed: false }]);
    expect(() => npmNotices(missing, accepted)).toThrow(/node_modules\/gone: not installed/);
  });

  it("names the package.json it cannot read", () => {
    const ui = tree([{ name: "broken", license: "MIT", files: { LICENSE: "x" } }]);
    writeFileSync(join(ui, "node_modules", "broken", "package.json"), "{");
    expect(() => npmNotices(ui, accepted)).toThrow(/reading .*node_modules\/broken\/package\.json: /);
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
