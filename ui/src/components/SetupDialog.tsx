import { useEffect, useState } from "react";
import type { Api, Project, SetupCheck, SetupReport } from "../api";

interface Props {
  api: Api;
  project: Project;
  onClose: () => void;
}

const LEVELS: { level: SetupCheck["level"]; heading: string }[] = [
  { level: "required", heading: "Needed for any run" },
  { level: "needed-for-pr", heading: "Needed to open a pull request" },
  { level: "optional", heading: "Needed by some runs" },
];

type Checked = { report: SetupReport } | { error: string } | null;

/** What the project needs for a run, checked where its agents run, grouped by how much each check matters, a failed
 * one with its fix. Closing it records that setup was checked when every required check passed, so the app does not
 * open it by itself again; a failure to record that is shown, and closing once more closes without it. */
export function SetupDialog({ api, project, onClose }: Props) {
  // Which round of checks is shown: "Check again" starts the next.
  const [round, setRound] = useState(0);
  // What the round's checks gave, by the round they answer, so a round in progress shows as checking.
  const [checked, setChecked] = useState<{ round: number; result: Checked }>({ round: -1, result: null });
  const [markError, setMarkError] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    api.setupChecks(project).then(
      (report) => current && setChecked({ round, result: { report } }),
      (error: Error) => current && setChecked({ round, result: { error: error.message } }),
    );
    return () => {
      current = false;
    };
  }, [api, project, round]);
  const result = checked.round === round ? checked.result : null;
  const close = () => {
    if (markError !== null || !(result && "report" in result && result.report.passed)) {
      onClose();
      return;
    }
    api.markSetupChecked().then(onClose, (error: Error) => setMarkError(error.message));
  };
  return (
    <div className="dialog-backdrop">
      <section className="setup" role="dialog" aria-modal="true" aria-label="setup">
        <h2>Setup</h2>
        <p className="where">
          {project.cwd} {project.machine ? `on ${project.machine}` : "on this computer"}
        </p>
        {result === null && <p className="loading">Checking…</p>}
        {result && "error" in result && <p className="problem">{result.error}</p>}
        {result &&
          "report" in result &&
          LEVELS.map(({ level, heading }) => (
            <CheckGroup key={level} heading={heading} checks={result.report.checks.filter((c) => c.level === level)} />
          ))}
        {markError !== null && <p className="problem">Could not record that setup was checked: {markError}</p>}
        <footer>
          <button type="button" onClick={() => setRound((r) => r + 1)} disabled={result === null}>
            Check again
          </button>
          <button type="button" onClick={close}>
            Close
          </button>
        </footer>
      </section>
    </div>
  );
}

/** The checks of one level, each with ok, missing or warn as `orchestrator doctor` words them; nothing without any. */
function CheckGroup({ heading, checks }: { heading: string; checks: SetupCheck[] }) {
  if (checks.length === 0) {
    return null;
  }
  return (
    <section aria-label={heading}>
      <h3>{heading}</h3>
      <ul className="checks">
        {checks.map((c) => {
          const word = c.ok ? "ok" : c.level === "optional" ? "warn" : "missing";
          return (
            <li key={c.name} data-state={word}>
              <span className="word">{word}</span> <span className="name">{c.name}</span>: {c.detail}
              {c.fix && <p className="fix">fix: {c.fix}</p>}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
