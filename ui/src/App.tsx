import { useEffect, useMemo, useState } from "react";
import { Api, type Connection, type Project } from "./api";
import { ProjectBar } from "./components/ProjectBar";
import { RunDetail } from "./components/RunDetail";
import { RunForm } from "./components/RunForm";
import { RunsOverview } from "./components/RunsOverview";
import { SetupDialog } from "./components/SetupDialog";
import { useNow, useRunStream } from "./hooks";
import { runsNewestFirst } from "./runs";

// The project last opened, kept per viewer across starts.
const PROJECT_KEY = "orchestrator.project";

/** The project last opened, else fallback's; a project with an empty cwd when there is neither. */
function savedProject(fallback: string | null): Project {
  try {
    const saved = JSON.parse(localStorage.getItem(PROJECT_KEY) ?? "null") as Project | null;
    if (saved && typeof saved.cwd === "string" && typeof saved.machine === "string") {
      return saved;
    }
  } catch {
    // Storage may be unavailable or hold something else; the fallback will do.
  }
  return { cwd: fallback ?? "", machine: "" };
}

function saveProject(project: Project): void {
  try {
    localStorage.setItem(PROJECT_KEY, JSON.stringify(project));
  } catch {
    // Remembering the project is a convenience.
  }
}

/** The app: the logo and name, then the project bar and the project's runs or one run in detail. connect resolves
 * to the sidecar's server, or rejects with why it could not be started. */
export function App({ connect }: { connect: () => Promise<Connection> }) {
  const [connection, setConnection] = useState<Connection | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => {
    connect().then(setConnection, (error: unknown) => setFailure(String(error)));
  }, [connect]);
  let body;
  if (failure) {
    body = <p className="problem">Could not start the orchestrator: {failure}</p>;
  } else {
    body = connection ? <Connected connection={connection} /> : <p className="loading">Starting the orchestrator…</p>;
  }
  return (
    <>
      <header className="app-header">
        {/* public/logo.svg is docs/logo.svg, held equal by logo.test.tsx; alt is that file's <title>. */}
        <img src="/logo.svg" alt="An octopus holding a terminal, a robot and a chat bubble" />
        <span className="app-name">Octoflow AI</span>
      </header>
      {body}
    </>
  );
}

function Connected({ connection }: { connection: Connection }) {
  const api = useMemo(() => new Api(connection), [connection]);
  const [project, setProject] = useState(() => savedProject(connection.project));
  const [settingUp, setSettingUp] = useState(false);
  // On start, with a project, the setup dialog opens by itself until it has been closed with every required check
  // passed. When the server cannot tell whether it has, the dialog stays closed: "Check setup…" still opens it.
  const [startedWithProject] = useState(project.cwd !== "");
  useEffect(() => {
    if (!startedWithProject) {
      return;
    }
    let current = true;
    api.setupChecked().then(
      (checked) => current && !checked && setSettingUp(true),
      () => {},
    );
    return () => {
      current = false;
    };
  }, [api, startedWithProject]);
  const open = (next: Project) => {
    saveProject(next);
    setProject(next);
  };
  return (
    <main>
      <ProjectBar project={project} onOpen={open} onCheckSetup={() => setSettingUp(true)} />
      {project.cwd ? (
        // Keyed by the project, so opening another one starts from its overview.
        <ProjectRuns key={`${project.machine}:${project.cwd}`} api={api} project={project} />
      ) : (
        <p className="empty">Open a project folder to see its runs.</p>
      )}
      {settingUp && project.cwd && (
        // After the runs, so that it is in front of a run form open beside it. Keyed by the project, so opening
        // another one while it is open checks that one.
        <SetupDialog key={`setup:${project.machine}:${project.cwd}`} api={api} project={project} onClose={() => setSettingUp(false)} />
      )}
    </main>
  );
}

/** One project's runs, live: the overview, with the form for a new run, or the run picked from it, or started, in
 * detail. A selected run the stream has not listed shows as the overview until it does, with a word on it when it
 * is the run just started. */
function ProjectRuns({ api, project }: { api: Api; project: Project }) {
  const [selected, setSelected] = useState<string | null>(null);
  const [formOpen, setFormOpen] = useState(false);
  // The id of the run last started from the form; null before one is.
  const [started, setStarted] = useState<string | null>(null);
  const state = useRunStream(api, project);
  const now = useNow();
  const entry = selected ? state.runs[selected] : undefined;
  const openStarted = (run: string) => {
    setFormOpen(false);
    setStarted(run);
    setSelected(run);
  };
  return (
    <>
      {state.problem && <p className="problem">{state.problem}</p>}
      {formOpen && <RunForm api={api} project={project} onStarted={openStarted} onClose={() => setFormOpen(false)} />}
      {entry ? (
        <RunDetail key={entry.record.id} api={api} project={project} entry={entry} now={now} onClose={() => setSelected(null)} />
      ) : (
        <>
          <div className="runs-actions">
            <button type="button" onClick={() => setFormOpen(true)}>
              New run…
            </button>
            {selected !== null && selected === started && (
              <>
                <p className="loading">
                  Run {selected} has started but is not listed yet; it opens once it is. If it never is, its herdr
                  workspace shows why.
                </p>
                <button type="button" onClick={() => setSelected(null)}>
                  Stop waiting
                </button>
              </>
            )}
          </div>
          <RunsOverview runs={runsNewestFirst(state)} now={now} selected={selected} onSelect={setSelected} />
        </>
      )}
    </>
  );
}
