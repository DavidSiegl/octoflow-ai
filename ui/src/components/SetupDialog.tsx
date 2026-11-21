import { useEffect, useState } from "react";
import type { Api, GateFile, Project, SetupCheck, SetupReport } from "../api";
import { Dialog } from "./Dialog";

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

/** What setting up the quality gate gave: the files it looked after, the error it failed with, or null while it runs. */
type GateOutcome = { files: GateFile[] } | { error: string } | null;

/** What installing an ACP adapter gave: the error it failed with, or null while it runs. */
type InstallOutcome = { error: string } | null;

/** What the project needs for a run, checked where its agents run, grouped by how much each check matters, a failed
 * one with its fix; a button that sets the project up for the local quality gate, showing which files it wrote
 * and which it kept; and, for each ACP adapter the report offers and has not installed, a button that installs it,
 * after which the checks run again. Closing it records that setup was checked when every required check passed,
 * which keeps the app from opening it by itself as Api.setupChecked says; a failure to record that is shown, and
 * closing once more closes without it. */
export function SetupDialog({ api, project, onClose }: Props) {
  // Which round of checks is shown: "Check again" starts the next.
  const [round, setRound] = useState(0);
  // What the latest round's checks gave, with the round they answer, so a round in progress shows as checking.
  const [answered, setAnswered] = useState<{ round: number; outcome: Outcome }>({ round: -1, outcome: null });
  // True while Close waits for the marker to be written, so a second click sends no second write.
  const [marking, setMarking] = useState(false);
  const [markError, setMarkError] = useState<string | null>(null);
  // The quality gate's setup: undefined until asked for, then as GateOutcome says.
  const [gate, setGate] = useState<GateOutcome | undefined>(undefined);
  // The latest install of each ACP adapter, by agent, as InstallOutcome says; an agent is missing until its install
  // is asked for, and again once it succeeds or a round reports its adapter installed.
  const [installs, setInstalls] = useState<Record<string, InstallOutcome>>({});
  useEffect(() => {
    let current = true;
    api.setupChecks(project).then(
      (report) => {
        if (!current) {
          return;
        }
        setAnswered({ round, outcome: { report } });
        // An install that failed, here or before another window installed it, is past once the adapter is there.
        const installed = new Set(report.adapters.filter((a) => a.installed).map((a) => a.agent));
        setInstalls((all) => withoutKeys(all, (agent, o) => o !== null && installed.has(agent)));
      },
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
  const setUpGate = () => {
    setGate(null);
    api.setUpGate(project).then(
      (files) => setGate({ files }),
      (error: Error) => setGate({ error: error.message }),
    );
  };
  const install = (agent: string) => {
    setInstalls((all) => ({ ...all, [agent]: null }));
    api.installAdapter(agent).then(
      () => {
        setInstalls((all) => withoutKeys(all, (a) => a === agent));
        setRound((r) => r + 1);
      },
      (error: Error) => setInstalls((all) => ({ ...all, [agent]: { error: error.message } })),
    );
  };
  // The adapters with a button: each the report offers and has not installed, and each with an install asked for,
  // which outlives a round in progress; the report's megabytes where it has them.
  const offered = outcome && "report" in outcome ? outcome.report.adapters : [];
  const megabytes = new Map(offered.map((a) => [a.agent, a.megabytes]));
  const shown = [...offered.filter((a) => !a.installed).map((a) => a.agent), ...Object.keys(installs)];
  const agents = [...new Set(shown)];
  return (
    <Dialog label="setup" title="Setup" project={project} className="setup">
      {outcome === null && <p className="loading">Checking…</p>}
      {outcome && "error" in outcome && <p className="problem">{outcome.error}</p>}
      {outcome &&
        "report" in outcome &&
        LEVELS.map(({ level, heading }) => (
          <CheckGroup key={level} heading={heading} checks={outcome.report.checks.filter((c) => c.level === level)} />
        ))}
      {gate !== undefined && <GateSetup outcome={gate} />}
      {Object.entries(installs).map(([agent, o]) => (
        <AdapterInstall key={agent} agent={agent} outcome={o} />
      ))}
      {markError !== null && <p className="problem">Could not record that setup was checked: {markError}</p>}
      <footer>
        <button type="button" onClick={() => setRound((r) => r + 1)} disabled={outcome === null || marking}>
          Check again
        </button>
        <button type="button" onClick={setUpGate} disabled={gate === null || marking}>
          Set up the quality gate
        </button>
        {agents.map((agent) => (
          <button
            key={agent}
            type="button"
            onClick={() => install(agent)}
            disabled={installs[agent] === null || marking}
          >
            Install {agent}'s ACP adapter{megabytes.has(agent) && ` (about ${megabytes.get(agent)} MB)`}
          </button>
        ))}
        <button type="button" onClick={close} disabled={marking}>
          Close
        </button>
      </footer>
    </Dialog>
  );
}

/** The checks of one level, each with ok, missing or warn as `octoflow doctor` words them; nothing without any. */
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

/** What setting up the local quality gate gave: a line for each file, wrote or kept as `octoflow doctor
 * --setup-gate` words them, and that they count once committed; the error it failed with; or that it runs. */
function GateSetup({ outcome }: { outcome: GateOutcome }) {
  return (
    <section aria-label="Quality gate">
      <h3>Quality gate</h3>
      {outcome === null && <p className="loading">Setting up…</p>}
      {outcome && "error" in outcome && <p className="problem">{outcome.error}</p>}
      {outcome && "files" in outcome && (
        <>
          <ul className="checks">
            {outcome.files.map((f) => (
              <li key={f.path} data-state={f.written ? "ok" : "warn"}>
                <span className="word">{f.written ? "wrote" : "kept"}</span> <span className="name">{f.path}</span>
                {!f.written && ": already there"}
              </li>
            ))}
          </ul>
          <p>
            <code>--quality-gate local</code> reads these from a run's base commit: commit them for a run to be judged
            by them.
          </p>
        </>
      )}
    </section>
  );
}

/** record without the entries drop picks. */
function withoutKeys<T>(record: Record<string, T>, drop: (key: string, value: T) => boolean): Record<string, T> {
  return Object.fromEntries(Object.entries(record).filter(([k, v]) => !drop(k, v)));
}

/** What installing agent's ACP adapter gave: the error it failed with, or that it runs. */
function AdapterInstall({ agent, outcome }: { agent: string; outcome: InstallOutcome }) {
  return (
    <section aria-label={`${agent}'s ACP adapter`}>
      <h3>{agent}'s ACP adapter</h3>
      {outcome === null && <p className="loading">Installing…</p>}
      {outcome && <p className="problem">{outcome.error}</p>}
    </section>
  );
}
