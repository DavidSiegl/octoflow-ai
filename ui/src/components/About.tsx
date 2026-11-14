import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { Dialog } from "./Dialog";

// What each entry opens is the Tauri shell's: `open_about` takes the name of an AboutLink (ui/src-tauri/src/lib.rs),
// never a URL or a path.
const PAGES = [
  { label: "Source code", link: "source" },
  { label: "Releases", link: "releases" },
  { label: "Support on Ko-fi", link: "ko-fi" },
];
const LICENSE_FILES = [
  { label: "Third-party licenses: crates", link: "crate-licenses" },
  { label: "Third-party licenses: npm packages", link: "npm-licenses" },
];

/** The About dialog: the app's version, copyright and license, and links to the repository, its releases, Ko-fi and
 * the bundled third-party licenses, each opened outside the app. A development build bundles no license files, so
 * it says so in their place. version is the release version (`Connection.version`); onClose is called on Close or
 * Escape. */
export function About({ version, onClose }: { version: string; onClose: () => void }) {
  // Why the link clicked last could not be opened; null while it has not failed.
  const [failure, setFailure] = useState<string | null>(null);
  // How many links have been clicked: an open that fails after a later click says nothing.
  const clicks = useRef(0);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => event.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const open = (link: string, label: string) => {
    const click = ++clicks.current;
    setFailure(null);
    invoke("open_about", { link }).catch((error: unknown) => {
      if (click === clicks.current) setFailure(`Could not open ${label}: ${String(error)}`);
    });
  };
  // Buttons rather than <a href>: a link's href is one the webview could follow itself, into the app's own window.
  const entry = ({ label, link }: { label: string; link: string }) => (
    <button type="button" className="link" onClick={() => open(link, label)}>
      {label}
    </button>
  );
  return (
    <Dialog label="about" title="Octoflow AI" className="about">
      <p>Version {version}</p>
      <p>© 2026 David Siegl</p>
      <p>Licensed under MIT OR Apache-2.0.</p>
      <ul className="links">
        {PAGES.map((page) => (
          <li key={page.link}>{entry(page)}</li>
        ))}
        {LICENSE_FILES.map((file) => (
          <li key={file.link}>
            {import.meta.env.DEV ? <>{file.label}: not bundled in a development build</> : entry(file)}
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
