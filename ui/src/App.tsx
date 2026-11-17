import { useEffect, useMemo, useRef, useState } from "react";
import { Api, type Connection, type Project, type StartedRun } from "./api";
import { About } from "./components/About";
import { CleanUpDialog } from "./components/CleanUpDialog";
import { ForgetDialog } from "./components/ForgetDialog";
import { ProjectBar } from "./components/ProjectBar";
import { RunDetail } from "./components/RunDetail";
import { RunForm } from "./components/RunForm";
import { RunsOverview } from "./components/RunsOverview";
import { SetupDialog } from "./components/SetupDialog";
import { WorkflowsDialog } from "./components/WorkflowsDialog";
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

/** The app: the top bar (logo and name, which open About, the project bar, and Workflows…), and the project's runs or one run
 * in detail. connect resolves to the sidecar's server, or rejects with why it could not be started. */
export function App({ connect }: { connect: () => Promise<Connection> }) {
  const [connection, setConnection] = useState<Connection | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => {
    connect().then(setConnection, (error: unknown) => setFailure(String(error)));
  }, [connect]);
  if (failure) {
    return <p className="problem">Could not start the orchestrator: {failure}</p>;
  }
  return connection ? <Connected connection={connection} /> : <p className="loading">Starting the orchestrator…</p>;
}

function Connected({ connection }: { connection: Connection }) {
  const api = useMemo(() => new Api(connection), [connection]);
  const [project, setProject] = useState(() => savedProject(connection.project));
  const [settingUp, setSettingUp] = useState(false);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [workflowsOpen, setWorkflowsOpen] = useState(false);
  // The logo's button, which gets the focus back when About closes.
  const badge = useRef<HTMLButtonElement>(null);
  const closeAbout = () => {
    setAboutOpen(false);
    badge.current?.focus();
  };
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
      <header className="top-bar">
        <button
          ref={badge}
          type="button"
          className="app-badge"
          aria-label="About Octoflow AI"
          onClick={() => setAboutOpen(true)}
        >
          {/* public/logo.svg is docs/logo.svg, held equal by logo.test.tsx; alt is that file's <title>. */}
          <img src="/logo.svg" alt="An octopus holding a terminal, a robot and a chat bubble" />
          <span className="app-name">Octoflow AI</span>
        </button>
        <ProjectBar project={project} onOpen={open} onCheckSetup={() => setSettingUp(true)} />
        {/* Beside the project bar, not in it: workflows are this computer's, not a project's. */}
        <button type="button" className="workflows-button" onClick={() => setWorkflowsOpen(true)}>
          Workflows…
        </button>
      </header>
      {project.cwd ? (
        // Keyed by the project, so opening another one starts from its overview.
        <ProjectRuns key={`${project.machine}:${project.cwd}`} api={api} project={project} />
      ) : (
        <p className="empty">Open a project folder to see its runs.</p>
      )}
      {settingUp && project.cwd && (
        // Keyed by the project, so that a dialog left open across a change of project checks the new one.
        <SetupDialog key={`setup:${project.machine}:${project.cwd}`} api={api} project={project} onClose={() => setSettingUp(false)} />
      )}
      {workflowsOpen && <WorkflowsDialog api={api} onClose={() => setWorkflowsOpen(false)} />}
      {aboutOpen && <About version={connection.version} onClose={closeAbout} />}
    </main>
  );
}

/** One project's runs, live: the overview, with the form for a new run and the dialogs that clean up runs and delete
 * closed ones, or the run picked from it, or started, in detail. A selected run the stream has not listed shows as the
 * overview until it does, with a word on it when it is the run just started. */
function ProjectRuns({ api, project }: { api: Api; project: Project }) {
  const [selected, setSelected] = useState<string | null>(null);
  const [formOpen, setFormOpen] = useState(false);
  const [cleaningUp, setCleaningUp] = useState(false);
  const [forgetting, setForgetting] = useState(false);
  // Whether the overview lists closed runs. Kept here, not in the overview, so that it lasts while a run is shown in
  // detail; it starts false each time a project opens, as ProjectRuns is keyed by the project.
  const [showClosed, setShowClosed] = useState(false);
  // The run last started from the form; null before one is.
  const [started, setStarted] = useState<StartedRun | null>(null);
  const state = useRunStream(api, project);
  const now = useNow();
  const entry = selected ? state.runs[selected] : undefined;
  const openStarted = (run: StartedRun) => {
    setFormOpen(false);
    setStarted(run);
    setSelected(run.run);
  };
  return (
    <>
      {state.problem && <p className="problem">{state.problem}</p>}
      {formOpen && <RunForm api={api} project={project} onStarted={openStarted} onClose={() => setFormOpen(false)} />}
      {cleaningUp && <CleanUpDialog api={api} project={project} runs={state.runs} onClose={() => setCleaningUp(false)} />}
      {forgetting && <ForgetDialog api={api} project={project} runs={state.runs} onClose={() => setForgetting(false)} />}
      {entry ? (
        <RunDetail key={entry.record.id} api={api} project={project} entry={entry} now={now} onClose={() => setSelected(null)} />
      ) : (
        <>
          <div className="runs-actions">
            <button type="button" onClick={() => setFormOpen(true)}>
              New run…
            </button>
            <button type="button" onClick={() => setCleaningUp(true)}>
              Clean up runs…
            </button>
            {selected !== null && selected === started?.run && (
              <>
                <p className="loading">
                  Run {selected} has started but is not listed yet; it opens once it is. If it never is,{" "}
                  {started.herdrSession ? (
                    <>
                      <code>herdr session attach {started.herdrSession}</code> shows why.
                    </>
                  ) : (
                    "its herdr workspace shows why."
                  )}
                </p>
                <button type="button" onClick={() => setSelected(null)}>
                  Stop waiting
                </button>
              </>
            )}
          </div>
          <RunsOverview
            runs={runsNewestFirst(state)}
            now={now}
            selected={selected}
            onSelect={setSelected}
            showClosed={showClosed}
            onShowClosed={setShowClosed}
            onForget={() => setForgetting(true)}
          />
        </>
      )}
    </>
  );
}
