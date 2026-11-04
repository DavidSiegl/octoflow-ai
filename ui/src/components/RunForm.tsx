import { type FormEvent, useEffect, useState } from "react";
import type { Api, OfferedRole, OfferedWorkflow, Project, RunOptions } from "../api";
import { Dialog } from "./Dialog";

interface Props {
  api: Api;
  project: Project;
  /** Gets the id of the run once the server has started it. */
  onStarted: (run: string) => void;
  /** Called by Cancel, which is disabled while the run is being started. */
  onClose: () => void;
}

/** The workflow a run takes unless another is chosen, as `run` takes it without --workflow. */
const DEFAULT_WORKFLOW = "default";

/** DEFAULT_WORKFLOW where it is among workflows and loads; null, for one to be picked, where not. */
function defaultIfLoads(workflows: OfferedWorkflow[]): string | null {
  return workflows.some((w) => w.name === DEFAULT_WORKFLOW && "roles" in w) ? DEFAULT_WORKFLOW : null;
}

/** What a role's row sets for it: an agent, one of the harnesses offered, and a model; each empty for not set, which
 * leaves the role to the run's agent and model, else to its workflow's own. */
interface RoleChoice {
  agent: string;
  model: string;
}

/** The roles of the workflow named name among workflows, in their order; none for one that is not there or does not
 * load. */
function rolesOf(workflows: OfferedWorkflow[] | undefined, name: string | null): OfferedRole[] {
  const found = workflows?.find((w) => w.name === name);
  return found && "roles" in found ? found.roles : [];
}

/** The value of field that choices set, by role, a model trimmed; undefined where none is set, for the request to
 * leave it out. */
function setByRole(choices: Map<string, RoleChoice>, field: keyof RoleChoice): Record<string, string> | undefined {
  const set = [...choices].flatMap(([role, choice]) => (choice[field].trim() ? [[role, choice[field].trim()]] : []));
  return set.length > 0 ? Object.fromEntries(set) : undefined;
}

/** A form for a new run in project: its task, workflow, agent and model, an agent and a model for each role of the
 * workflow, and whether it opens a pull request and runs in a worktree of its own. Start starts it; an error the
 * server answers stays in the form, with what was entered, until the next Start. */
export function RunForm({ api, project, onStarted, onClose }: Props) {
  // What /workflows gave, the error that kept it from giving it, or null until it answers.
  const [offered, setOffered] = useState<{ options: RunOptions } | { error: string } | null>(null);
  const [task, setTask] = useState("");
  // The workflow picked; null until one is.
  const [picked, setPicked] = useState<string | null>(null);
  // An agent and a model for the run's roles that their rows leave empty; empty for each role's default.
  const [agent, setAgent] = useState("");
  const [model, setModel] = useState("");
  // What each role's row sets, by role key; only roles of the workflow chosen have an entry, so that a change of
  // workflow keeps the rows of the roles both share and empties the others. A Map, since a role key such as
  // `constructor` names a member every object has.
  const [choices, setChoices] = useState<Map<string, RoleChoice>>(new Map());
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
  // The workflow Start sends: the one picked, else DEFAULT_WORKFLOW where it loads. When /workflows failed, which a
  // workflows directory that cannot be listed makes it do, DEFAULT_WORKFLOW, as `run`, which still offers the built-in
  // ones then, takes it. null while the workflows are read, and while one is to be picked.
  let workflow: string | null = null;
  if (options) {
    workflow = picked ?? defaultIfLoads(options.workflows);
  } else if (offered) {
    workflow = DEFAULT_WORKFLOW;
  }
  const chosen = options?.workflows.find((w) => w.name === workflow);
  const roles = rolesOf(options?.workflows, workflow);
  const pick = (name: string) => {
    setPicked(name);
    const shared = new Set(rolesOf(options?.workflows, name).map((r) => r.role));
    setChoices((old) => new Map([...old].filter(([role]) => shared.has(role))));
  };
  const choose = (role: string, change: Partial<RoleChoice>) =>
    setChoices((old) => new Map(old).set(role, { agent: "", model: "", ...old.get(role), ...change }));
  // What a role's row left empty falls back to, as `run` takes it: the run's agent and model, else the role's default,
  // which is the workflow's, or for an agent the workflow does not name, the built-in one (/workflows does not say
  // which).
  const agentFallback = (r: OfferedRole) => (agent ? `${agent} (the run's)` : `${r.agent} (default)`);
  const modelFallback = (r: OfferedRole) => {
    if (model.trim()) {
      return `${model.trim()} (the run's)`;
    }
    return r.model ? `${r.model} (default)` : "the harness's own";
  };
  const kinds = options?.agents.map((kind) => (
    <option key={kind} value={kind}>
      {kind}
    </option>
  ));
  const ready = task.trim() !== "" && workflow !== null && !starting;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    // workflow's test narrows it for the request; ready holds it too.
    if (!ready || workflow === null) {
      return;
    }
    setStarting(true);
    setProblem(null);
    const request = {
      task: task.trim(),
      workflow,
      agent: agent || undefined,
      model: model.trim() || undefined,
      role_agents: setByRole(choices, "agent"),
      role_models: setByRole(choices, "model"),
      no_pr: noPr,
      worktree,
    };
    api.startRun(project, request).then(onStarted, (error: Error) => {
      setStarting(false);
      setProblem(error.message);
    });
  };
  return (
    <Dialog label="new run" title="New run" project={project} className="run-form">
      <form onSubmit={submit}>
        <fieldset disabled={starting}>
          <label>
            Task
            <textarea value={task} onChange={(e) => setTask(e.target.value)} rows={4} />
          </label>
          <label>
            Workflow
            <select value={workflow ?? ""} onChange={(e) => pick(e.target.value)} disabled={!options}>
              {workflow === null && (
                <option value="" disabled>
                  {options ? "pick one" : ""}
                </option>
              )}
              {!options && workflow !== null && <option value={workflow}>{workflow}</option>}
              {options?.workflows.map((w) => (
                <option key={w.name} value={w.name} disabled={!("roles" in w)} title={"error" in w ? w.error : undefined}>
                  {"error" in w ? `${w.name} (does not load)` : w.name}
                </option>
              ))}
            </select>
          </label>
          {offered === null && <p className="loading">Reading the workflows…</p>}
          {offered && "error" in offered && (
            <p className="problem">
              {offered.error}; the run takes the {DEFAULT_WORKFLOW} workflow
            </p>
          )}
          {chosen && "description" in chosen && chosen.description && <p className="description">{chosen.description}</p>}
          <div className="row">
            <label>
              Agent
              <select value={agent} onChange={(e) => setAgent(e.target.value)}>
                <option value="">the workflow&apos;s</option>
                {kinds}
              </select>
            </label>
            <label>
              Model
              <input value={model} onChange={(e) => setModel(e.target.value)} placeholder="the workflow's" />
            </label>
          </div>
          {roles.map((r) => {
            const choice = choices.get(r.role);
            return (
              <div key={r.role} role="group" aria-label={`role ${r.label}`} className="row role">
                <span className="role-label">{r.label}</span>
                <select
                  aria-label={`${r.label} agent`}
                  className={choice?.agent ? undefined : "unset"}
                  value={choice?.agent ?? ""}
                  onChange={(e) => choose(r.role, { agent: e.target.value })}
                >
                  <option value="">{agentFallback(r)}</option>
                  {kinds}
                </select>
                <input
                  aria-label={`${r.label} model`}
                  value={choice?.model ?? ""}
                  onChange={(e) => choose(r.role, { model: e.target.value })}
                  placeholder={modelFallback(r)}
                />
              </div>
            );
          })}
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
          <button type="submit" disabled={!ready}>
            Start
          </button>
        </footer>
      </form>
    </Dialog>
  );
}
