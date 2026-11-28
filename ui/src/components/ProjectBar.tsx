import { invoke } from "@tauri-apps/api/core";
import { homeDir } from "@tauri-apps/api/path";
import { useState, type FormEvent } from "react";
import type { Project } from "../api";

interface Props {
  /** The project open now. */
  project: Project;
  /** Gets the project the fields name when the form is submitted. */
  onOpen: (project: Project) => void;
  /** Called by "Check setup…", which checks the project open now, not what the fields hold. */
  onCheckSetup: () => void;
}

/** The folder the system's folder dialog starts in: the open project's, its `~` replaced by the home directory, which
 * the dialog does not know; the home directory when no project is open. */
async function dialogStart(cwd: string): Promise<string> {
  if (cwd !== "" && cwd !== "~" && !cwd.startsWith("~/")) {
    return cwd;
  }
  const home = (await homeDir()).replace(/\/$/, "");
  return home + cwd.slice(1);
}

/** The project folder field, typed or pasted into, with Open, and Choose…, which picks a folder in the system's folder
 * dialog and opens it as Open does; and the button that checks the open project's setup. While the dialog is open,
 * neither Open nor Choose… can be used, so that one dialog is open at a time and its pick is the last project opened. */
export function ProjectBar({ project, onOpen, onCheckSetup }: Props) {
  const [cwd, setCwd] = useState(project.cwd);
  // Why the folder dialog could not be shown; null while it has not failed since a project was last opened from the bar
  // or Choose… was last clicked.
  const [failure, setFailure] = useState<string | null>(null);
  // True while the folder dialog is open. On Linux it is not modal to the window (tauri-plugin-dialog parents it only
  // on Windows and macOS), so the bar has to keep its own buttons from being used meanwhile.
  const [choosing, setChoosing] = useState(false);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!choosing) {
      setFailure(null);
      onOpen({ cwd: cwd.trim() });
    }
  };
  const choose = async () => {
    setFailure(null);
    setChoosing(true);
    try {
      // The dialog plugin's own command, as its npm package's open() sends it; that package is not a dependency, as it
      // ships no license text the app's bundle could carry (ui/scripts/npm-licenses.js). null when cancelled.
      const options = { directory: true, defaultPath: await dialogStart(project.cwd) };
      const picked = await invoke<string | null>("plugin:dialog|open", { options });
      if (picked !== null) {
        setCwd(picked);
        onOpen({ cwd: picked });
      }
    } catch (error) {
      setFailure(String(error));
    } finally {
      setChoosing(false);
    }
  };
  return (
    <form className="project-bar" onSubmit={submit}>
      <label>
        Project folder
        <input value={cwd} onChange={(e) => setCwd(e.target.value)} placeholder="~/GitHub/myproject" />
      </label>
      <button type="submit" disabled={!cwd.trim() || choosing}>
        Open
      </button>
      <button type="button" disabled={choosing} onClick={() => void choose()}>
        Choose…
      </button>
      <button type="button" onClick={onCheckSetup} disabled={!project.cwd}>
        Check setup…
      </button>
      {failure && <span className="problem">Could not show the folder dialog: {failure}</span>}
    </form>
  );
}
