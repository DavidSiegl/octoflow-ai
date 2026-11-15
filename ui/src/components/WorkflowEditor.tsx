import { useCallback, useEffect, useMemo, useState } from "react";
import {
  type Api,
  type EditableWorkflow,
  HttpError,
  STEP_FLAGS,
  type StepDefinition,
  type WorkflowCheck,
  type WorkflowDefinition,
  type WorkflowFile,
} from "../api";
import { Dialog } from "./Dialog";

/** How long after the last change the editor asks the server to check the definition, in milliseconds. */
export const CHECK_DELAY_MS = 300;

/** What the editor opens: workflow `edit` as it is (read-only when it is built in), or a copy of workflow `copy`
 * under a name still to choose, which is saved as a new file. */
export type Opening = { edit: string } | { copy: string };

/** A role as its row edits it: its key and every field, an empty agent or model for none. */
interface DraftRole {
  key: string;
  label: string;
  agent: string;
  model: string;
}

/** A step as the editor edits it: every field present, an empty again, fresh_note or loop_to for not set. */
type DraftStep = Required<StepDefinition>;

/** A workflow as the form holds it: every field as its widget holds it, text where a definition has a number. */
interface Draft {
  name: string;
  description: string;
  maxFiles: string;
  maxLines: string;
  proofCommand: string;
  roles: DraftRole[];
  steps: DraftStep[];
}

/** The workflow the editor holds, once read: its file (null for a built-in workflow or a copy, which have none),
 * whether it is a built-in one, which is read-only, and the draft it opened with, which tells an edit apart. */
interface Opened {
  file: WorkflowFile | null;
  builtIn: boolean;
  initial: Draft;
}

function draftStep(step: StepDefinition): DraftStep {
  const flags = Object.fromEntries(STEP_FLAGS.map((flag) => [flag, step[flag] === true]));
  return { ...step, again: step.again ?? "", fresh_note: step.fresh_note ?? "", loop_to: step.loop_to ?? "", ...flags } as DraftStep;
}

/** definition under name, as the form holds it. */
function draftOf(name: string, definition: WorkflowDefinition): Draft {
  return {
    name,
    description: definition.description ?? "",
    maxFiles: definition.max_files?.toString() ?? "",
    maxLines: definition.max_lines?.toString() ?? "",
    proofCommand: definition.proof_command ?? "",
    roles: Object.entries(definition.roles).map(([key, r]) => ({ key, label: r.label, agent: r.agent ?? "", model: r.model ?? "" })),
    steps: definition.steps.map(draftStep),
  };
}

/** A budget field's text as a definition holds it: absent when empty, a number when it is one, else the text, for
 * the server to name as the problem. */
function budget(text: string): number | string | undefined {
  const trimmed = text.trim();
  if (!trimmed) {
    return undefined;
  }
  return /^\d+$/.test(trimmed) ? Number(trimmed) : trimmed;
}

/** The definition draft holds, in the canonical form a save writes: a field at its default left out. A role key given
 * twice keeps its last row, which draftProblem names first. */
function definitionOf(draft: Draft): WorkflowDefinition {
  const optional = <T,>(key: string, value: T | undefined | "") => (value === undefined || value === "" ? {} : { [key]: value });
  const steps = draft.steps.map((s) => {
    const flags = Object.fromEntries(STEP_FLAGS.filter((flag) => s[flag]).map((flag) => [flag, true]));
    return {
      id: s.id,
      role: s.role,
      file: s.file,
      prompt: s.prompt,
      ...optional("again", s.again),
      ...optional("fresh_note", s.fresh_note),
      ...flags,
      ...optional("loop_to", s.loop_to),
    };
  });
  return {
    ...optional("description", draft.description),
    ...optional("max_files", budget(draft.maxFiles)),
    ...optional("max_lines", budget(draft.maxLines)),
    ...optional("proof_command", draft.proofCommand),
    roles: Object.fromEntries(
      draft.roles.map((r) => [r.key, { label: r.label, ...optional("agent", r.agent), ...optional("model", r.model) }]),
    ),
    steps,
  } as WorkflowDefinition;
}

/** What the server cannot be asked about, since a definition's roles, a JSON object, cannot hold it: two roles under
 * one key. null when there is none. */
function draftProblem(draft: Draft): string | null {
  const seen = new Set<string>();
  for (const { key } of draft.roles) {
    if (seen.has(key)) {
      return `two roles have the key ${key || "(empty)"}`;
    }
    seen.add(key);
  }
  return null;
}

/** A confirmation the editor waits for: saving over a hand-written file, closing with unsaved changes, or reloading
 * the file, which discards the edits. */
type Asking = "hand-written" | "close" | "reload" | null;

interface Props {
  api: Api;
  /** The harnesses a role's agent can be, as /workflows offers them. */
  agents: string[];
  opening: Opening;
  /** Gets the workflow's name once it is saved; the editor is done then. */
  onSaved: (name: string) => void;
  /** Called when the editor closes without saving: on Close with no unsaved changes, or on Discard. */
  onClose: () => void;
}

/** An editor of a workflow file, over the window: its name (for a copy), description, diff budget and proof command;
 * its roles, each with its label, agent and model; and its steps, each with every field. The server checks the
 * definition a moment after each change, and Save is enabled only while its last check of what the form holds found
 * no problem. A save names the file's text the editor opened, so that the server refuses it when the file changed
 * since; a hand-written file is rewritten only after a confirmation. A built-in workflow opens read-only, with
 * Duplicate… in place of Save. Every confirmation is a dialog of its own, in front. */
export function WorkflowEditor({ api, agents, opening, onSaved, onClose }: Props) {
  // The workflow as read; null while it is, or a load that failed: the server's error, or a file that does not load.
  const [opened, setOpened] = useState<Opened | null>(null);
  const [unloaded, setUnloaded] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  // The step whose fields show, by index into draft.steps.
  const [current, setCurrent] = useState(0);
  // The server's last check, with the key (checkKey) of the draft it checked.
  const [check, setCheck] = useState<(WorkflowCheck & { key: string }) | null>(null);
  const [asking, setAsking] = useState<Asking>(null);
  const [saving, setSaving] = useState(false);
  // Why the last save failed, and whether because the file changed (409), which Reload answers; null when it did not.
  const [failure, setFailure] = useState<{ message: string; conflict: boolean } | null>(null);

  const load = useCallback(
    (from: Opening) => {
      const name = "edit" in from ? from.edit : from.copy;
      const take = (workflow: EditableWorkflow) => {
        if ("error" in workflow) {
          setUnloaded(`${workflow.path}: ${workflow.error}`);
          return;
        }
        const copy = "copy" in from;
        const initial = draftOf(copy ? "" : name, workflow.definition);
        setOpened({ file: copy ? null : workflow.file, builtIn: !copy && workflow.file === null, initial });
        setDraft(initial);
        setCurrent(0);
        setFailure(null);
      };
      return api.workflow(name).then(take, (error: Error) => setUnloaded(error.message));
    },
    [api],
  );
  useEffect(() => {
    load(opening);
  }, [load, opening]);

  const definition = useMemo(() => (draft ? definitionOf(draft) : null), [draft]);
  // The name and definition the server is asked about, as one string, to tell which draft a check answers.
  const checkKey = draft ? JSON.stringify([draft.name, definition]) : "";
  const localProblem = draft ? draftProblem(draft) : null;
  useEffect(() => {
    if (!draft || !definition || localProblem) {
      return;
    }
    let live = true;
    const timer = setTimeout(() => {
      api.checkWorkflow(draft.name, definition).then(
        (found) => live && setCheck({ ...found, key: checkKey }),
        (error: Error) => live && setCheck({ problem: `could not check the workflow: ${error.message}`, placeholders: [], key: checkKey }),
      );
    }, CHECK_DELAY_MS);
    return () => {
      live = false;
      clearTimeout(timer);
    };
    // checkKey stands for draft.name and definition.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, checkKey, localProblem]);

  if (unloaded !== null || !opened || !draft || !definition) {
    return (
      <Dialog label="workflow" title={"edit" in opening ? opening.edit : `Copy of ${opening.copy}`} className="workflow-editor">
        {unloaded === null ? <p className="loading">Reading the workflow…</p> : <p className="problem">{unloaded}</p>}
        <footer>
          <button type="button" onClick={onClose}>
            Close
          </button>
        </footer>
      </Dialog>
    );
  }

  const readOnly = opened.builtIn;
  const checked = check?.key === checkKey ? check : null;
  const problem = localProblem ?? checked?.problem ?? null;
  const canSave = !readOnly && !saving && localProblem === null && checked !== null && checked.problem === null;
  const changed = JSON.stringify(draft) !== JSON.stringify(opened.initial);

  const edit = (change: Partial<Draft>) => setDraft({ ...draft, ...change });
  const editRole = (i: number, change: Partial<DraftRole>) => {
    const old = draft.roles[i];
    if (!old) {
      return;
    }
    const roles = draft.roles.map((r, j) => (j === i ? { ...r, ...change } : r));
    // A renamed role takes its steps along.
    const renamed = change.key !== undefined && change.key !== old.key;
    const steps = renamed ? draft.steps.map((s) => (s.role === old.key ? { ...s, role: change.key as string } : s)) : draft.steps;
    edit({ roles, steps });
  };
  const editStep = (change: Partial<DraftStep>) => {
    const old = draft.steps[current];
    if (!old) {
      return;
    }
    // A renamed step takes the loops back to it along.
    const renamed = change.id !== undefined && change.id !== old.id;
    const steps = draft.steps.map((s, j) => {
      const next = j === current ? { ...s, ...change } : s;
      return renamed && next.loop_to === old.id ? { ...next, loop_to: change.id as string } : next;
    });
    edit({ steps });
  };
  const addStep = () => {
    const step = draftStep({ id: `step${draft.steps.length + 1}`, role: draft.roles[0]?.key ?? "", file: "", prompt: "" });
    edit({ steps: [...draft.steps, step] });
    setCurrent(draft.steps.length);
  };
  const removeStep = () => {
    edit({ steps: draft.steps.filter((_, j) => j !== current) });
    setCurrent(Math.max(0, Math.min(current, draft.steps.length - 2)));
  };
  const moveStep = (by: -1 | 1) => {
    const to = current + by;
    const [moved, other] = [draft.steps[current], draft.steps[to]];
    if (!moved || !other) {
      return;
    }
    edit({ steps: draft.steps.map((s, j) => (j === current ? other : j === to ? moved : s)) });
    setCurrent(to);
  };

  const save = (confirmed: boolean) => {
    if (!canSave) {
      return;
    }
    if (opened.file?.handWritten && !confirmed) {
      setAsking("hand-written");
      return;
    }
    setAsking(null);
    setSaving(true);
    setFailure(null);
    api.saveWorkflow(draft.name, definition, opened.file?.text ?? null).then(
      () => onSaved(draft.name),
      (error: Error) => {
        setSaving(false);
        setFailure({ message: error.message, conflict: error instanceof HttpError && error.status === 409 });
      },
    );
  };
  const close = () => (changed && !readOnly ? setAsking("close") : onClose());
  const duplicate = () => {
    setOpened({ file: null, builtIn: false, initial: { ...draft, name: "" } });
    setDraft({ ...draft, name: "" });
  };
  const reload = () => {
    setAsking(null);
    if ("edit" in opening || draft.name) {
      load({ edit: draft.name });
    }
  };

  const step = draft.steps[current];
  const roleKeys = draft.roles.map((r) => r.key);
  const placeholders = (checked ?? check)?.placeholders ?? [];
  const nameFixed = opened.file !== null || readOnly;
  return (
    <>
      <Dialog
        label="workflow"
        title={readOnly ? `${draft.name} (built in, read-only)` : draft.name || "New workflow"}
        className="workflow-editor"
      >
        {opened.file && <p className="where">{opened.file.path}</p>}
        <fieldset disabled={readOnly || saving}>
          {!nameFixed && (
            <label>
              Name
              <input value={draft.name} onChange={(e) => edit({ name: e.target.value })} placeholder="letters, digits, - and _" />
            </label>
          )}
          <label>
            Description
            <input value={draft.description} onChange={(e) => edit({ description: e.target.value })} />
          </label>
          <div className="row">
            <label>
              max_files
              <input value={draft.maxFiles} inputMode="numeric" onChange={(e) => edit({ maxFiles: e.target.value })} placeholder="default" />
            </label>
            <label>
              max_lines
              <input value={draft.maxLines} inputMode="numeric" onChange={(e) => edit({ maxLines: e.target.value })} placeholder="default" />
            </label>
            <label>
              proof_command
              <input value={draft.proofCommand} onChange={(e) => edit({ proofCommand: e.target.value })} placeholder="none" />
            </label>
          </div>

          <h3>Roles</h3>
          <table className="roles">
            <thead>
              <tr>
                <th>Key</th>
                <th>Label</th>
                <th>Agent</th>
                <th>Model</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {draft.roles.map((r, i) => (
                // By index: the key is what the row edits.
                <tr key={i}>
                  <td>
                    <input aria-label={`role ${i + 1} key`} value={r.key} onChange={(e) => editRole(i, { key: e.target.value })} />
                  </td>
                  <td>
                    <input aria-label={`role ${i + 1} label`} value={r.label} onChange={(e) => editRole(i, { label: e.target.value })} />
                  </td>
                  <td>
                    <select aria-label={`role ${i + 1} agent`} value={r.agent} onChange={(e) => editRole(i, { agent: e.target.value })}>
                      <option value="">the run&apos;s, else the default</option>
                      {[...new Set([...agents, r.agent].filter(Boolean))].map((kind) => (
                        <option key={kind} value={kind}>
                          {kind}
                        </option>
                      ))}
                    </select>
                  </td>
                  <td>
                    <input
                      aria-label={`role ${i + 1} model`}
                      value={r.model}
                      onChange={(e) => editRole(i, { model: e.target.value })}
                      placeholder="the harness's own"
                    />
                  </td>
                  <td>
                    <button type="button" onClick={() => edit({ roles: draft.roles.filter((_, j) => j !== i) })}>
                      Remove
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="actions">
            <button type="button" onClick={() => edit({ roles: [...draft.roles, { key: "", label: "", agent: "", model: "" }] })}>
              Add role
            </button>
          </div>

          <h3>Steps</h3>
          <div className="steps">
            <ul aria-label="steps">
              {draft.steps.map((s, i) => (
                <li key={i}>
                  <button type="button" aria-pressed={i === current} onClick={() => setCurrent(i)}>
                    {s.id || "(no id)"}
                  </button>
                </li>
              ))}
            </ul>
            <div className="actions">
              <button type="button" onClick={addStep}>
                Add step
              </button>
              <button type="button" onClick={removeStep} disabled={!step}>
                Remove step
              </button>
              <button type="button" onClick={() => moveStep(-1)} disabled={current === 0}>
                Up
              </button>
              <button type="button" onClick={() => moveStep(1)} disabled={current >= draft.steps.length - 1}>
                Down
              </button>
            </div>
          </div>
          {step && (
            <div role="group" aria-label={`step ${step.id}`} className="step">
              <div className="row">
                <label>
                  id
                  <input value={step.id} onChange={(e) => editStep({ id: e.target.value })} />
                </label>
                <label>
                  role
                  <select value={step.role} onChange={(e) => editStep({ role: e.target.value })}>
                    {[...new Set([...roleKeys, step.role])].map((key) => (
                      <option key={key} value={key}>
                        {key || "(none)"}
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  file
                  <input value={step.file} onChange={(e) => editStep({ file: e.target.value })} />
                </label>
                <label>
                  loop_to
                  <select value={step.loop_to} onChange={(e) => editStep({ loop_to: e.target.value })}>
                    <option value="">none</option>
                    {draft.steps
                      .filter((s, i) => i !== current && s.id)
                      .map((s) => (
                        <option key={s.id} value={s.id}>
                          {s.id}
                        </option>
                      ))}
                  </select>
                </label>
              </div>
              <div className="prompts">
                <div className="texts">
                  <label>
                    prompt
                    <textarea value={step.prompt} rows={8} onChange={(e) => editStep({ prompt: e.target.value })} />
                  </label>
                  <label>
                    again
                    <textarea value={step.again} rows={3} onChange={(e) => editStep({ again: e.target.value })} placeholder="not set" />
                  </label>
                  <label>
                    fresh_note
                    <textarea value={step.fresh_note} rows={3} onChange={(e) => editStep({ fresh_note: e.target.value })} placeholder="not set" />
                  </label>
                </div>
                <aside aria-label="placeholders" className="placeholders">
                  <p>Placeholders the prompts may use:</p>
                  <ul>
                    {placeholders.map((p) => (
                      <li key={p}>
                        <code>{`{${p}}`}</code>
                      </li>
                    ))}
                  </ul>
                </aside>
              </div>
              <div className="flags">
                {STEP_FLAGS.map((flag) => (
                  <label key={flag} className="check">
                    <input type="checkbox" checked={step[flag]} onChange={(e) => editStep({ [flag]: e.target.checked })} />
                    {flag}
                  </label>
                ))}
              </div>
            </div>
          )}
        </fieldset>
        {!readOnly && problem !== null && <pre className="problem">{problem}</pre>}
        {saving && <p className="loading">Saving…</p>}
        {failure && (
          <div className="problem">
            <pre>{failure.message}</pre>
            {failure.conflict && opened.file && (
              <button type="button" onClick={() => setAsking("reload")}>
                Reload
              </button>
            )}
          </div>
        )}
        <footer>
          <button type="button" onClick={close} disabled={saving}>
            Close
          </button>
          {readOnly ? (
            <button type="button" onClick={duplicate}>
              Duplicate…
            </button>
          ) : (
            <button type="button" onClick={() => save(false)} disabled={!canSave}>
              Save
            </button>
          )}
        </footer>
      </Dialog>
      {asking === "hand-written" && opened.file && (
        <Dialog label="rewrite hand-written file" title="Rewrite a hand-written file?" className="confirm">
          <p>
            {opened.file.path} was written by hand; saving rewrites it without its comments, and its <code>use</code> lines
            become the steps they name, with the built-in prompts&apos; text.
          </p>
          <footer>
            <button type="button" onClick={() => setAsking(null)}>
              Cancel
            </button>
            <button type="button" onClick={() => save(true)}>
              Save
            </button>
          </footer>
        </Dialog>
      )}
      {asking === "close" && (
        <Dialog label="unsaved changes" title="Unsaved changes" className="confirm">
          <p>{draft.name || "The new workflow"} has changes that are not saved.</p>
          <footer>
            <button type="button" onClick={() => setAsking(null)}>
              Cancel
            </button>
            <button type="button" onClick={onClose}>
              Discard
            </button>
            <button type="button" onClick={() => save(false)} disabled={!canSave}>
              Save
            </button>
          </footer>
        </Dialog>
      )}
      {asking === "reload" && opened.file && (
        <Dialog label="reload workflow" title="Reload the workflow?" className="confirm">
          <p>Reading {opened.file.path} again discards the changes made here.</p>
          <footer>
            <button type="button" onClick={() => setAsking(null)}>
              Cancel
            </button>
            <button type="button" onClick={reload}>
              Reload
            </button>
          </footer>
        </Dialog>
      )}
    </>
  );
}
