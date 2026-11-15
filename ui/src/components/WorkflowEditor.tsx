import { useCallback, useEffect, useMemo, useState } from "react";
import {
  type Api,
  type EditableWorkflow,
  HttpError,
  STEP_FLAGS,
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

/** The number of the last row made: each role and step row gets the next, which names it for as long as the editor
 * holds it, whatever its key or id is edited to. */
let lastRow = 0;
const nextRow = () => ++lastRow;

/** A role as its row edits it: the row's number, its key and every field, an empty agent or model for none. */
interface DraftRole {
  row: number;
  key: string;
  label: string;
  agent: string;
  model: string;
}

type Flags = Record<(typeof STEP_FLAGS)[number], boolean>;

/** A step as the editor edits it: its row's number, and every field, an empty again or fresh_note for not set. Its
 * role and the step it loops back to are named by their rows, not by key and id, so that renaming one does not lose
 * the steps that name it; a row no longer in the draft is one that was removed. loopTo null is no loop. */
type DraftStep = {
  row: number;
  id: string;
  role: number;
  file: string;
  prompt: string;
  again: string;
  fresh_note: string;
  loopTo: number | null;
} & Flags;

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

/** A step of no flags and no texts, its own row, its role's row role and id id. */
function newStep(id: string, role: number): DraftStep {
  const flags = Object.fromEntries(STEP_FLAGS.map((flag) => [flag, false])) as Flags;
  return { row: nextRow(), id, role, file: "", prompt: "", again: "", fresh_note: "", loopTo: null, ...flags };
}

/** definition under name, as the form holds it. A step's role that the definition's roles lack, which a definition
 * read from the server never has, gets a row of its own. */
function draftOf(name: string, definition: WorkflowDefinition): Draft {
  const roles = Object.entries(definition.roles).map(([key, r]) => ({
    row: nextRow(),
    key,
    label: r.label,
    agent: r.agent ?? "",
    model: r.model ?? "",
  }));
  const roleRow = (key: string) => {
    const found = roles.find((r) => r.key === key) ?? { row: nextRow(), key, label: key, agent: "", model: "" };
    if (!roles.includes(found)) {
      roles.push(found);
    }
    return found.row;
  };
  const steps = definition.steps.map((s) => {
    const flags = Object.fromEntries(STEP_FLAGS.map((flag) => [flag, s[flag] === true])) as Flags;
    return { ...newStep(s.id, roleRow(s.role)), file: s.file, prompt: s.prompt, again: s.again ?? "", fresh_note: s.fresh_note ?? "", ...flags };
  });
  definition.steps.forEach((s, i) => {
    const step = steps[i];
    if (step) {
      step.loopTo = steps.find((t) => s.loop_to !== undefined && t.id === s.loop_to)?.row ?? null;
    }
  });
  return {
    name,
    description: definition.description ?? "",
    maxFiles: definition.max_files?.toString() ?? "",
    maxLines: definition.max_lines?.toString() ?? "",
    proofCommand: definition.proof_command ?? "",
    roles,
    steps,
  };
}

/** A budget field's text as a definition holds it: absent when empty, a number when it is a whole one JSON carries
 * exactly, else the text, for the server to name as the problem. */
function budget(text: string): number | string | undefined {
  const trimmed = text.trim();
  if (!trimmed) {
    return undefined;
  }
  return /^\d+$/.test(trimmed) && Number.isSafeInteger(Number(trimmed)) ? Number(trimmed) : trimmed;
}

/** The definition draft holds, in the canonical form a save writes: a field at its default left out. A role key given
 * twice keeps its last row, and a removed role or loop target is written as none; draftProblem names both first. */
function definitionOf(draft: Draft): WorkflowDefinition {
  const optional = <T,>(key: string, value: T | undefined | "") => (value === undefined || value === "" ? {} : { [key]: value });
  const steps = draft.steps.map((s) => {
    const flags = Object.fromEntries(STEP_FLAGS.filter((flag) => s[flag]).map((flag) => [flag, true]));
    return {
      id: s.id,
      role: draft.roles.find((r) => r.row === s.role)?.key ?? "",
      file: s.file,
      prompt: s.prompt,
      ...optional("again", s.again),
      ...optional("fresh_note", s.fresh_note),
      ...flags,
      ...optional("loop_to", draft.steps.find((t) => t.row === s.loopTo)?.id),
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

/** What the server cannot be asked about, since a definition cannot hold it: two roles under one key, which its
 * roles' JSON object would merge, and a step whose role or loop target was removed, which it would name by a key or
 * id that may since have been given to another. null when there is none. */
function draftProblem(draft: Draft): string | null {
  const seen = new Set<string>();
  for (const { key } of draft.roles) {
    if (seen.has(key)) {
      return `two roles have the key ${key || "(empty)"}`;
    }
    seen.add(key);
  }
  for (const s of draft.steps) {
    if (!draft.roles.some((r) => r.row === s.role)) {
      return `step ${s.id || "(no id)"} has no role: pick one`;
    }
    if (s.loopTo !== null && !draft.steps.some((t) => t.row === s.loopTo)) {
      return `step ${s.id || "(no id)"} loops back to a step that was removed: pick another, or none`;
    }
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
  const editRole = (i: number, change: Partial<DraftRole>) =>
    edit({ roles: draft.roles.map((r, j) => (j === i ? { ...r, ...change } : r)) });
  const editStep = (change: Partial<DraftStep>) => edit({ steps: draft.steps.map((s, j) => (j === current ? { ...s, ...change } : s)) });
  const addStep = () => {
    let n = draft.steps.length + 1;
    while (draft.steps.some((s) => s.id === `step${n}`)) {
      n++;
    }
    // Row 0 is no role's, when there is none to give it.
    edit({ steps: [...draft.steps, newStep(`step${n}`, draft.roles[0]?.row ?? 0)] });
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
    load(opening);
  };

  const step = draft.steps[current];
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
                <tr key={r.row}>
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
            <button type="button" onClick={() => edit({ roles: [...draft.roles, { row: nextRow(), key: "", label: "", agent: "", model: "" }] })}>
              Add role
            </button>
          </div>

          <h3>Steps</h3>
          <div className="steps">
            <ul aria-label="steps">
              {draft.steps.map((s, i) => (
                <li key={s.row}>
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
                  <select value={step.role} onChange={(e) => editStep({ role: Number(e.target.value) })}>
                    {!draft.roles.some((r) => r.row === step.role) && <option value={step.role}>(removed)</option>}
                    {draft.roles.map((r) => (
                      <option key={r.row} value={r.row}>
                        {r.key || "(no key)"}
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
                  <select
                    value={step.loopTo ?? ""}
                    onChange={(e) => editStep({ loopTo: e.target.value === "" ? null : Number(e.target.value) })}
                  >
                    <option value="">none</option>
                    {step.loopTo !== null && !draft.steps.some((s) => s.row === step.loopTo) && <option value={step.loopTo}>(removed)</option>}
                    {/* Another step: a step cannot loop back to itself. */}
                    {draft.steps
                      .filter((s) => s.row !== step.row)
                      .map((s) => (
                        <option key={s.row} value={s.row}>
                          {s.id || "(no id)"}
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
