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

/** What a round of checks gave: the report, the error that kept the server from giving one, or null while it runs. */
type Outcome = { report: SetupReport } | { error: string } | null;

/** What the project needs for a run, checked where its agents run, grouped by how much each check matters, a failed
 * one with its fix. Closing it records that setup was checked when every required check passed, so the app does not
 * open it by itself again; a failure to record that is shown, and closing once more closes without it. */
export function SetupDialog({ api, project, onClose }: Props) {
  // Which round of checks is shown: "Check again" starts the next.
  const [round, setRound] = useState(0);
  // What the latest round's checks gave, with the round they answer, so a round in progress shows as checking.
  const [answered, setAnswered] = useState<{ round: number; outcome: Outcome }>({ round: -1, outcome: null });
  // True while Close waits for the marker to be written, so a second click sends no second write.
  const [marking, setMarking] = useState(false);
  const [markError, setMarkError] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    api.setupChecks(project).then(
      (report) => current && setAnswered({ round, outcome: { report } }),
      (error: Error) => current && setAnswered({ round, outcome: { error: error.message } }),
    );
    return () => {
      current = false;
    };
  }, [api, project, round]);
  const outcome = answered.round === round ? answered.outcome : null;
  const close = () => {
    if (markError !== null || !(outcome && "report" in outcome && outcome.report.passed)) {
      onClose();
      return;
    }
    setMarking(true);
    api.markSetupChecked().then(onClose, (error: Error) => {
      setMarking(false);
      setMarkError(error.message);
    });
  };
  return (
    <div className="dialog-backdrop">
      <section className="dialog setup" role="dialog" aria-modal="true" aria-label="setup">
        <h2>Setup</h2>
        <p className="where">
          {project.cwd} {project.machine ? `on ${project.machine}` : "on this computer"}
        </p>
        {outcome === null && <p className="loading">Checking…</p>}
        {outcome && "error" in outcome && <p className="problem">{outcome.error}</p>}
        {outcome &&
          "report" in outcome &&
          LEVELS.map(({ level, heading }) => (
            <CheckGroup key={level} heading={heading} checks={outcome.report.checks.filter((c) => c.level === level)} />
          ))}
        {markError !== null && <p className="problem">Could not record that setup was checked: {markError}</p>}
        <footer>
          <button type="button" onClick={() => setRound((r) => r + 1)} disabled={outcome === null || marking}>
            Check again
          </button>
          <button type="button" onClick={close} disabled={marking}>
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
