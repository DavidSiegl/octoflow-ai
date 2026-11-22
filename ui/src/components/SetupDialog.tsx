import { useEffect, useRef, useState } from "react";
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

/** How many askings in a row may fail before the dialog stops following an install and says why. */
const LOOK_TRIES = 5;

/** What the project needs for a run, checked where its agents run, grouped by how much each check matters, a failed
 * one with its fix; a button that sets the project up for the local quality gate, showing which files it wrote
 * and which it kept; and, for each ACP adapter the report offers and has not installed, a button that starts an
 * install of it on the server. An install the server runs, started here or before the dialog opened, shows as
 * installing, followed every INSTALL_POLL_MS until it ends; then the checks run again if it installed the adapter,
 * and the error it failed with, or what it left in the installs' directory, shows. Closing it records that setup was
 * checked when every required check passed, which keeps the app from opening it by itself as Api.setupChecked says;
 * a failure to record that is shown, and closing once more closes without it. */
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
  // How many times the server was asked for the adapters, by a round or by following an install.
  const asked = useRef(0);
  // By agent, how many times the server had been asked for the adapters when it said it started an install of it
  // here: an answer to one of those askings may be older than the install, and is not taken for that agent.
  const startedAfter = useRef<Record<string, number>>({});
  useEffect(() => {
    let current = true;
    const sent = ++asked.current;
    api.setupChecks(project).then(
      (report) => {
        if (current) {
          setAnswered({ round, outcome: { report } });
          setAdapters((all) => newer(all, report.adapters, sent, startedAfter.current));
        }
      },
      (error: Error) => current && setAnswered({ round, outcome: { error: error.message } }),
    );
    return () => {
      current = false;
    };
  }, [api, project, round]);
  // Follow the installs that run until each ends, starting again when which run changes.
  const followed = adapters
    .filter((a) => a.installing)
    .map((a) => a.agent)
    .join(" ");
  useEffect(() => {
    if (followed === "") {
      return;
    }
    const agents = followed.split(" ");
    let current = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let failures = 0;
    const look = () => {
      const sent = ++asked.current;
      api.setupAdapters().then(
        (next) => {
          if (!current) {
            return;
          }
          failures = 0;
          setAdapters((all) => newer(all, next, sent, startedAfter.current));
          const ended = (a: AdapterInstall) =>
            agents.includes(a.agent) && fresh(sent, a.agent, startedAfter.current) && !a.installing && a.installed;
          if (next.some(ended)) {
            setRound((r) => r + 1);
          }
          timer = setTimeout(look, INSTALL_POLL_MS);
        },
        // A look that fails leaves the adapters as they were, and looks again, LOOK_TRIES times in a row at most.
        (error: Error) => {
          if (!current) {
            return;
          }
          failures += 1;
          if (failures < LOOK_TRIES) {
            timer = setTimeout(look, INSTALL_POLL_MS);
            return;
          }
          const lost = { installing: false, error: `cannot tell how the install goes: ${error.message}` };
          setAdapters((all) => all.map((a) => (agents.includes(a.agent) ? { ...a, ...lost } : a)));
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
    api
      .installAdapter(agent)
      .then(
        () => {
          startedAfter.current[agent] = asked.current;
          changed({ installing: true, error: null, warning: null });
        },
        (error: Error) => changed({ error: error.message }),
      )
      .finally(() => setStarting((all) => all.filter((a) => a !== agent)));
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

/** Whether the server's answer to asking number sent is newer than any install of agent the dialog started, by
 * startedAfter (by agent, the number of askings when the server said it started one). */
function fresh(sent: number, agent: string, startedAfter: Record<string, number>): boolean {
  return sent > (startedAfter[agent] ?? 0);
}

/** next, the adapters the server answered asking number sent with, but for each agent it is not fresh for, whose
 * entry in all, the dialog's, is newer. */
function newer(
  all: AdapterInstall[],
  next: AdapterInstall[],
  sent: number,
  startedAfter: Record<string, number>,
): AdapterInstall[] {
  return next.map((a) => (fresh(sent, a.agent, startedAfter) ? a : (all.find((o) => o.agent === a.agent) ?? a)));
}

/** How the latest install of an ACP adapter goes, when there is something to say: that it runs (busy), the error it
 * failed with while the adapter is still not installed, or what it could not remove from the installs' directory;
 * nothing else. */
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
      {left && <p>Installed, but not everything else in its installs' directory could be removed: {adapter.warning}</p>}
    </section>
  );
}
