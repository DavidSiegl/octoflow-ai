import { type RefObject, useEffect, useMemo, useRef, useState } from "react";
import { Api, type Connection, type Project } from "./api";
import { About } from "./components/About";
import { CleanUpDialog } from "./components/CleanUpDialog";
import { CloseGuard, type AppWindow } from "./components/CloseGuard";
import { ForgetDialog } from "./components/ForgetDialog";
import { ProjectBar } from "./components/ProjectBar";
import { RunDetail } from "./components/RunDetail";
import { RunForm } from "./components/RunForm";
import { RunsOverview } from "./components/RunsOverview";
import { SettingsDialog } from "./components/SettingsDialog";
import { SetupDialog } from "./components/SetupDialog";
import { WorkflowsDialog } from "./components/WorkflowsDialog";
import { useRunStream } from "./hooks";
import { runsNewestFirst } from "./runs";

// The project last opened, kept per viewer across starts.
const PROJECT_KEY = "orchestrator.project";

/** The project last opened, else fallback's (a project with an empty cwd when there is neither), and the herdr
 * machine it was saved with, which an app before 0.94.0 ran a project on and this one does not; null for none. */
function savedProject(fallback: string | null): { project: Project; machine: string | null } {
  try {
    const saved = JSON.parse(localStorage.getItem(PROJECT_KEY) ?? "null") as { cwd?: unknown; machine?: unknown } | null;
    if (saved && typeof saved.cwd === "string") {
      const machine = typeof saved.machine === "string" && saved.machine !== "" ? saved.machine : null;
      return { project: { cwd: saved.cwd }, machine };
    }
  } catch {
    // Storage may be unavailable or hold something else; the fallback will do.
  }
  return { project: { cwd: fallback ?? "" }, machine: null };
}

/** Save project as the one to reopen once the server says it can read it (Api.runsProblem), unless latest, when the
 * answer comes, no longer names project, as after another project opened since. A project the server refuses is no
 * longer saved, if it was, so that a folder deleted or moved since it was saved is not reopened at every start. An
 * answer that never comes changes nothing. */
function rememberIfRead(api: Api, project: Project, latest: RefObject<Project>): void {
  api.runsProblem(project).then(
    (why) => {
      if (latest.current !== project) {
        return;
      }
      if (why === null) {
        saveProject(project);
      } else if (savedCwd() === project.cwd) {
        forgetProject();
      }
    },
    () => {},
  );
}

/** The folder of the project saved as the one to reopen; "" when none is. */
function savedCwd(): string {
  return savedProject(null).project.cwd;
}

function forgetProject(): void {
  try {
    localStorage.removeItem(PROJECT_KEY);
  } catch {
    // Remembering the project is a convenience.
  }
}

function saveProject(project: Project): void {
  try {
    localStorage.setItem(PROJECT_KEY, JSON.stringify(project));
  } catch {
    // Remembering the project is a convenience.
  }
}

/** The app: the top bar (logo and name, which open About, the project bar, Workflows… and Settings…), and the project's runs or one run in detail. connect resolves to the sidecar's server, or rejects
 * with why it could not be started; appWindow is the window the app is in, whose close it guards while runs are
 * running (CloseGuard). */
export function App({ connect, appWindow }: { connect: () => Promise<Connection>; appWindow: AppWindow }) {
  const [connection, setConnection] = useState<Connection | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => {
    connect().then(setConnection, (error: unknown) => setFailure(String(error)));
  }, [connect]);
  if (failure) {
    return <p className="problem">Could not start the orchestrator: {failure}</p>;
  }
  return connection ? (
    <Connected connection={connection} appWindow={appWindow} />
  ) : (
    <p className="loading">Starting the orchestrator…</p>
  );
}

function Connected({ connection, appWindow }: { connection: Connection; appWindow: AppWindow }) {
  const api = useMemo(() => new Api(connection), [connection]);
  const [saved] = useState(() => savedProject(connection.project));
  const [project, setProject] = useState(saved.project);
  // The machine the project was saved with, while the note that the app runs no project on one is shown: until it is
  // dismissed or another project opens. Saved again without it, so the note shows once.
  const [droppedMachine, setDroppedMachine] = useState(saved.machine);
  useEffect(() => {
    if (saved.machine !== null) {
      saveProject(saved.project);
    }
  }, [saved]);
  const [settingUp, setSettingUp] = useState(false);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [workflowsOpen, setWorkflowsOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  // On start, with a project, the setup dialog opens by itself unless the server says it need not (setupChecked).
  // When the server cannot tell, the dialog stays closed: "Check setup…" still opens it.
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
  // The project opened last, the one shown at start until the user opens another: rememberIfRead saves only it.
  const latest = useRef(saved.project);
  // A saved project is checked again at start, so that one the server now refuses is forgotten; a project the app
  // fell back to, not saved, stays unsaved until the user opens it.
  useEffect(() => {
    if (saved.project.cwd !== "" && savedCwd() === saved.project.cwd) {
      rememberIfRead(api, saved.project, latest);
    }
  }, [api, saved]);
  const open = (next: Project) => {
    latest.current = next;
    rememberIfRead(api, next, latest);
    setProject(next);
    setDroppedMachine(null);
  };
  return (
    <main>
      <header className="top-bar">
        <button
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
        <button type="button" className="settings-button" onClick={() => setSettingsOpen(true)}>
          Settings…
        </button>
      </header>
      {droppedMachine !== null && (
        <p className="notice">
          This project was opened on the machine {droppedMachine}; the app now runs projects on this computer only, and
          opens {saved.project.cwd} here. Remote machines are a feature of octoflow's command line:{" "}
          <code>octoflow run --machine {droppedMachine}</code>.{" "}
          <button type="button" onClick={() => setDroppedMachine(null)}>
            Dismiss
          </button>
        </p>
      )}
      {project.cwd ? (
        // Keyed by the project, so opening another one starts from its overview.
        <ProjectRuns key={project.cwd} api={api} project={project} />
      ) : (
        <p className="empty">Open a project folder to see its runs.</p>
      )}
      {settingUp && project.cwd && (
        // Keyed by the project, so that a dialog left open across a change of project checks the new one.
        <SetupDialog key={`setup:${project.cwd}`} api={api} project={project} onClose={() => setSettingUp(false)} />
      )}
      {workflowsOpen && <WorkflowsDialog api={api} onClose={() => setWorkflowsOpen(false)} />}
      {aboutOpen && <About version={connection.version} onClose={() => setAboutOpen(false)} />}
      {settingsOpen && <SettingsDialog api={api} onClose={() => setSettingsOpen(false)} />}
      <CloseGuard api={api} project={project} appWindow={appWindow} />
    </main>
  );
}

/** One project's runs, live: the overview, with the form for a new run and the dialogs that clean up runs and delete
 * closed ones, or the run picked from it, or started, in detail. A selected run the stream has not listed shows as the
 * overview until it does, with a word on it when it is the run just started. While the server reports a problem with
 * the project, New run… and Clean up runs… are disabled. */
function ProjectRuns({ api, project }: { api: Api; project: Project }) {
  const [selected, setSelected] = useState<string | null>(null);
  const [formOpen, setFormOpen] = useState(false);
  const [cleaningUp, setCleaningUp] = useState(false);
  const [forgetting, setForgetting] = useState(false);
  // Whether the overview lists closed runs. Kept here, not in the overview, so that it lasts while a run is shown in
  // detail; it starts false each time a project opens, as ProjectRuns is keyed by the project.
  const [showClosed, setShowClosed] = useState(false);
  // The id of the run last started from the form; null before one is.
  const [started, setStarted] = useState<string | null>(null);
  const state = useRunStream(api, project);
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
      {cleaningUp && <CleanUpDialog api={api} project={project} runs={state.runs} onClose={() => setCleaningUp(false)} />}
      {forgetting && <ForgetDialog api={api} project={project} runs={state.runs} onClose={() => setForgetting(false)} />}
      {entry ? (
        <RunDetail key={entry.record.id} api={api} project={project} entry={entry} onClose={() => setSelected(null)} />
      ) : (
        <>
          <div className="runs-actions">
            <button type="button" disabled={state.problem !== null} onClick={() => setFormOpen(true)}>
              New run…
            </button>
            <button type="button" disabled={state.problem !== null} onClick={() => setCleaningUp(true)}>
              Clean up runs…
            </button>
            {selected !== null && selected === started && (
              <>
                <p className="loading">
                  Run {selected} has started but is not listed yet; it opens once it is. If it never is, its log,{" "}
                  <code>.octoflow-ai/runs/{selected}/run.log</code> in the project, shows why.
                </p>
                <button type="button" onClick={() => setSelected(null)}>
                  Stop waiting
                </button>
              </>
            )}
          </div>
          <RunsOverview
            runs={runsNewestFirst(state)}
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
