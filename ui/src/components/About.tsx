import { invoke } from "@tauri-apps/api/core";
import { resolveResource } from "@tauri-apps/api/path";
import { useEffect, useState, type MouseEvent } from "react";
import { Dialog } from "./Dialog";

// tauri-plugin-opener's commands (ui/src-tauri/src/lib.rs registers the plugin), called as its npm package,
// @tauri-apps/plugin-opener, calls them. The package itself is not used: it ships no license text, so
// scripts/npm-licenses.js could not ship it with the app.
const openUrl = (url: string) => invoke("plugin:opener|open_url", { url });
const openPath = (path: string) => invoke("plugin:opener|open_path", { path });

const REPOSITORY = "https://github.com/DavidSiegl/octoflow-ai";

// The pages About links to, opened in the system's browser. capabilities/default.json lets the opener open https
// URLs only.
const LINKS = [
  { label: "Source code", url: REPOSITORY },
  { label: "Releases", url: `${REPOSITORY}/releases` },
  { label: "Support on Ko-fi", url: "https://ko-fi.com/davidsiegl" },
];

// The third-party license files a packaged app bundles among its resources (packaging/build-desktop.sh), opened in
// the system's HTML viewer. capabilities/default.json lets the opener open these two and no other path.
const LICENSE_FILES = [
  { label: "Third-party licenses: crates", file: "THIRD-PARTY-LICENSES.html" },
  { label: "Third-party licenses: npm packages", file: "THIRD-PARTY-LICENSES-NPM.html" },
];

/** The About dialog: the app's version, copyright and license, and links to the repository, its releases, Ko-fi and
 * the bundled third-party licenses, each opened outside the app. A development build bundles no license files, so
 * it says so in their place. version is the release version (`Connection.version`); onClose is called on Close or
 * Escape. */
export function About({ version, onClose }: { version: string; onClose: () => void }) {
  // Why the last link clicked could not be opened; null while none has failed.
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => event.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const opened = (opening: Promise<unknown>, what: string) => {
    setFailure(null);
    opening.catch((error: unknown) => setFailure(`Could not open ${what}: ${String(error)}`));
  };
  const openLink = (event: MouseEvent, url: string) => {
    event.preventDefault();
    opened(openUrl(url), url);
  };
  const openLicenses = (file: string) => opened(resolveResource(file).then((path) => openPath(path)), file);
  return (
    <Dialog label="about" title="Octoflow AI" className="about">
      <p>Version {version}</p>
      <p>© 2026 David Siegl</p>
      <p>Licensed under MIT OR Apache-2.0.</p>
      <ul className="links">
        {LINKS.map(({ label, url }) => (
          <li key={url}>
            <a href={url} onClick={(event) => openLink(event, url)}>
              {label}
            </a>
          </li>
        ))}
        {LICENSE_FILES.map(({ label, file }) => (
          <li key={file}>
            {import.meta.env.DEV ? (
              <>
                {label}: not bundled in a development build
              </>
            ) : (
              <button type="button" className="link" onClick={() => openLicenses(file)}>
                {label}
              </button>
            )}
          </li>
        ))}
      </ul>
      {failure && <p className="problem">{failure}</p>}
      <footer>
        <button type="button" onClick={onClose} autoFocus>
          Close
        </button>
      </footer>
    </Dialog>
  );
}
