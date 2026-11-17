import { useEffect } from "react";
import { Dialog } from "./Dialog";
import { OutsideLink, type Outside } from "./OutsideLink";

const REPOSITORY = "https://github.com/DavidSiegl/octoflow-ai";
const PAGES: { label: string; target: Outside }[] = [
  { label: "Source code", target: { page: REPOSITORY } },
  { label: "Releases", target: { page: `${REPOSITORY}/releases` } },
  { label: "Support on Ko-fi", target: { page: "https://ko-fi.com/davidsiegl" } },
];
const LICENSE_FILES: { label: string; target: Outside }[] = [
  { label: "Third-party licenses: crates", target: { license: "crate-licenses" } },
  { label: "Third-party licenses: npm packages", target: { license: "npm-licenses" } },
];

/** The About dialog: the app's version, copyright and license, and links to the repository, its releases, Ko-fi and
 * the bundled third-party licenses, each opened outside the app. A development build bundles no license files, so
 * it says so in their place. version is the release version (`Connection.version`); onClose is called on Close or
 * Escape. */
export function About({ version, onClose }: { version: string; onClose: () => void }) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => event.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <Dialog label="about" title="Octoflow AI" className="about">
      <p>Version {version}</p>
      <p>© 2026 David Siegl</p>
      <p>Licensed under MIT OR Apache-2.0.</p>
      <ul className="links">
        {PAGES.map(({ label, target }) => (
          <li key={label}>
            <OutsideLink target={target}>{label}</OutsideLink>
          </li>
        ))}
        {LICENSE_FILES.map(({ label, target }) => (
          <li key={label}>
            {import.meta.env.DEV ? (
              <>{label}: not bundled in a development build</>
            ) : (
              <OutsideLink target={target}>{label}</OutsideLink>
            )}
          </li>
        ))}
      </ul>
      <footer>
        <button type="button" onClick={onClose} autoFocus>
          Close
        </button>
      </footer>
    </Dialog>
  );
}
