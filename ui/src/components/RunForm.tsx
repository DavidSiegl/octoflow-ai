import { type FormEvent, useEffect, useState } from "react";
import type { Api, OfferedWorkflow, Project, RunOptions, StartedRun } from "../api";

interface Props {
  api: Api;
  project: Project;
  /** Gets the run once the server has started it. */
  onStarted: (started: StartedRun) => void;
  /** Called by Cancel, which is disabled while the run is being started. */
  onClose: () => void;
}

/** The workflow a run takes unless another is chosen, as `run` takes it without --workflow. */
const DEFAULT_WORKFLOW = "default";

/** The workflow the form starts with: DEFAULT_WORKFLOW where it loads, else the first one that does; null for none. */
function firstChoice(workflows: OfferedWorkflow[]): string | null {
  const loadable = workflows.filter((w) => "roles" in w);
  return (loadable.find((w) => w.name === DEFAULT_WORKFLOW) ?? loadable[0])?.name ?? null;
}

/** A form for a new run in project: its task, workflow, agent and model, and whether it opens a pull request and
 * runs in a worktree of its own. Start starts it; an error the server answers stays in the form, with what was
 * entered, until the next Start. */
export function RunForm({ api, project, onStarted, onClose }: Props) {
  // What /workflows gave, the error that kept it from giving it, or null until it answers.
  const [offered, setOffered] = useState<{ options: RunOptions } | { error: string } | null>(null);
  const [task, setTask] = useState("");
  // The workflow picked; null until one is, the form then taking firstChoice's.
  const [picked, setPicked] = useState<string | null>(null);
  // An agent and a model for every role; empty for the workflow's own.
  const [agent, setAgent] = useState("");
  const [model, setModel] = useState("");
  const [noPr, setNoPr] = useState(false);
  const [worktree, setWorktree] = useState(true);
  // True from Start until the server answers: the form can then be neither changed, sent again nor closed.
  const [starting, setStarting] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    api.workflows().then(
      (options) => current && setOffered({ options }),
      (error: Error) => current && setOffered({ error: error.message }),
    );
    return () => {
      current = false;
    };
  }, [api]);
  const options = offered && "options" in offered ? offered.options : null;
  const workflow = picked ?? (options && firstChoice(options.workflows));
  const chosen = options?.workflows.find((w) => w.name === workflow);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!task.trim() || !workflow || starting) {
      return;
    }
    setStarting(true);
    setProblem(null);
    const request = {
      task: task.trim(),
      workflow,
      agent: agent || undefined,
      model: model.trim() || undefined,
      no_pr: noPr,
      worktree,
    };
    api.startRun(project, request).then(onStarted, (error: Error) => {
      setStarting(false);
      setProblem(error.message);
    });
  };
  return (
    <div className="dialog-backdrop">
      <form className="dialog run-form" role="dialog" aria-modal="true" aria-label="new run" onSubmit={submit}>
        <h2>New run</h2>
        <p className="where">
          {project.cwd} {project.machine ? `on ${project.machine}` : "on this computer"}
        </p>
        <fieldset disabled={starting}>
          <label>
            Task
            <textarea value={task} onChange={(e) => setTask(e.target.value)} rows={4} />
          </label>
          <label>
            Workflow
            <select value={workflow ?? ""} onChange={(e) => setPicked(e.target.value)} disabled={!options}>
              {options?.workflows.map((w) => (
                <option key={w.name} value={w.name} disabled={!("roles" in w)} title={"error" in w ? w.error : undefined}>
                  {"error" in w ? `${w.name} (does not load)` : w.name}
                </option>
              ))}
            </select>
          </label>
          {offered === null && <p className="loading">Reading the workflows…</p>}
          {offered && "error" in offered && <p className="problem">{offered.error}</p>}
          {chosen && "description" in chosen && chosen.description && <p className="description">{chosen.description}</p>}
          <div className="row">
            <label>
              Agent
              <select value={agent} onChange={(e) => setAgent(e.target.value)}>
                <option value="">the workflow&apos;s</option>
                {options?.agents.map((kind) => (
                  <option key={kind} value={kind}>
                    {kind}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Model
              <input value={model} onChange={(e) => setModel(e.target.value)} placeholder="the workflow's" />
            </label>
          </div>
          <label className="check">
            <input type="checkbox" checked={noPr} onChange={(e) => setNoPr(e.target.checked)} />
            No pull request
          </label>
          <label className="check">
            <input type="checkbox" checked={worktree} onChange={(e) => setWorktree(e.target.checked)} />
            Separate worktree
          </label>
        </fieldset>
        {starting && <p className="loading">Starting the run…</p>}
        {problem !== null && <pre className="problem">{problem}</pre>}
        <footer>
          <button type="button" onClick={onClose} disabled={starting}>
            Cancel
          </button>
          <button type="submit" disabled={!task.trim() || !workflow || starting}>
            Start
          </button>
        </footer>
      </form>
    </div>
  );
}
