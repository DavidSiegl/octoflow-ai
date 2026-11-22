import { useEffect, useState } from "react";
import type { AdapterInstall, Api, GateFile, Project, SetupCheck, SetupReport } from "../api";
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

/** How often the dialog asks how an install of an ACP adapter goes, while one runs, in milliseconds. */
export const INSTALL_POLL_MS = 500;

/** What the project needs for a run, checked where its agents run, grouped by how much each check matters, a failed
 * one with its fix; a button that sets the project up for the local quality gate, showing which files it wrote
 * and which it kept; and, for each ACP adapter the report offers and has not installed, a button that starts an
 * install of it on the server. An install the server runs, started here or before the dialog opened, shows as
 * installing, followed every INSTALL_POLL_MS until it ends; then the checks run again if it installed the adapter,
 * and the error it failed with, or the old installs it left, shows. Closing it records that setup was checked when every required check passed,
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
  // The ACP adapters as the server last told them, by the latest round's report or by following an install; empty
  // before the first report.
  const [adapters, setAdapters] = useState<AdapterInstall[]>([]);
  // The agents whose install was asked for here and has not yet started on the server.
  const [starting, setStarting] = useState<string[]>([]);
  useEffect(() => {
    let current = true;
    api.setupChecks(project).then(
      (report) => {
        if (current) {
          setAnswered({ round, outcome: { report } });
          setAdapters(report.adapters);
        }
      },
      (error: Error) => current && setAnswered({ round, outcome: { error: error.message } }),
    );
    return () => {
      current = false;
    };
  }, [api, project, round]);
  // Follow the installs that run until each ends, starting again when which run changes.
  const installing = adapters.filter((a) => a.installing).map((a) => a.agent);
  const followed = installing.join(" ");
  useEffect(() => {
    if (followed === "") {
      return;
    }
    let current = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const look = () => {
      // A look that fails leaves the adapters as they were, and looks again.
      api.setupAdapters().then(
        (next) => {
          if (!current) {
            return;
          }
          setAdapters(next);
          const agents = followed.split(" ");
          if (next.some((a) => agents.includes(a.agent) && !a.installing && a.installed)) {
            setRound((r) => r + 1);
          }
          timer = setTimeout(look, INSTALL_POLL_MS);
        },
        () => {
          if (current) {
            timer = setTimeout(look, INSTALL_POLL_MS);
          }
        },
      );
    };
    timer = setTimeout(look, INSTALL_POLL_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [api, followed]);
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
  // The install's start is the server's from here: it answers at once, and following the adapters shows the rest.
  const install = (agent: string) => {
    const changed = (fields: Partial<AdapterInstall>) =>
      setAdapters((all) => all.map((a) => (a.agent === agent ? { ...a, ...fields } : a)));
    setStarting((all) => [...all, agent]);
    api.installAdapter(agent).then(
      () => changed({ installing: true, error: null, warning: null }),
      (error: Error) => changed({ error: error.message }),
    ).finally(() => setStarting((all) => all.filter((a) => a !== agent)));
  };
  const busy = (a: AdapterInstall) => a.installing || starting.includes(a.agent);
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
      {adapters.map((a) => (
        <AdapterInstallation key={a.agent} adapter={a} busy={busy(a)} />
      ))}
      {markError !== null && <p className="problem">Could not record that setup was checked: {markError}</p>}
      <footer>
        <button type="button" onClick={() => setRound((r) => r + 1)} disabled={outcome === null || marking}>
          Check again
        </button>
        <button type="button" onClick={setUpGate} disabled={gate === null || marking}>
          Set up the quality gate
        </button>
        {adapters
          .filter((a) => !a.installed)
          .map((a) => (
            <button key={a.agent} type="button" onClick={() => install(a.agent)} disabled={busy(a) || marking}>
              Install {a.agent}'s ACP adapter (about {a.megabytes} MB)
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

/** How the latest install of an ACP adapter goes, when there is something to say: that it runs (busy), the error it
 * failed with while the adapter is still not installed, or the old installs it could not remove; nothing else. */
function AdapterInstallation({ adapter, busy }: { adapter: AdapterInstall; busy: boolean }) {
  const failed = !busy && !adapter.installed && adapter.error !== null;
  const left = !busy && adapter.warning !== null;
  if (!busy && !failed && !left) {
    return null;
  }
  return (
    <section aria-label={`${adapter.agent}'s ACP adapter`}>
      <h3>{adapter.agent}'s ACP adapter</h3>
      {busy && <p className="loading">Installing…</p>}
      {failed && <p className="problem">{adapter.error}</p>}
      {left && <p>Installed, but an old install is left, which the next install removes: {adapter.warning}</p>}
    </section>
  );
}
