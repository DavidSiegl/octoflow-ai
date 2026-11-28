import { useEffect, useState } from "react";
import { type Api, DEFAULT_WORKFLOW, type EditableWorkflow, type RunOptions } from "../api";
import { Dialog } from "./Dialog";
import { type Opening, WorkflowEditor } from "./WorkflowEditor";

/** A workflow of the list, as GET /workflow read it, or the error that kept it from being read; undefined while it
 * is. */
type Entry = EditableWorkflow | { name: string; unread: string } | undefined;

/** A workflow whose file is to be deleted: its name, path and text. */
interface DeleteTarget {
  name: string;
  path: string;
  text: string;
}

/** The workflows of this computer, as `run --workflow` offers them, each with its description, or for one whose file
 * does not load, its path and error. New opens the editor on a copy of the default workflow, Duplicate on a copy of
 * the entry, and Edit on the entry itself (View for a built-in one, which is read-only). Delete removes an entry's
 * file after a confirmation naming it; while it does, the entry shows "Deleting…" and offers no action, and the dialog
 * cannot be closed, so that a failure is seen. The workflow saved last is marked. */
export function WorkflowsDialog({ api, onClose }: { api: Api; onClose: () => void }) {
  // What /workflows gave, the error that kept it from giving it, or null until it answers.
  const [offered, setOffered] = useState<{ options: RunOptions } | { error: string } | null>(null);
  const [entries, setEntries] = useState<Record<string, Entry>>({});
  // Bumped to read the list again, after a save or a delete.
  const [generation, setGeneration] = useState(0);
  const [editing, setEditing] = useState<Opening | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  // The workflow whose delete waits on its confirmation.
  const [confirming, setConfirming] = useState<DeleteTarget | null>(null);
  // The names of the workflows whose delete was confirmed and has not been answered.
  const [deleting, setDeleting] = useState<ReadonlySet<string>>(new Set());
  // Why the last delete failed; null when it did not, or a save or delete came since.
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    api.workflows().then(
      (options) => {
        if (!current) {
          return;
        }
        setOffered({ options });
        for (const { name } of options.workflows) {
          api.workflow(name).then(
            (workflow) => current && setEntries((old) => ({ ...old, [name]: workflow })),
            (error: Error) => current && setEntries((old) => ({ ...old, [name]: { name, unread: error.message } })),
          );
        }
      },
      (error: Error) => current && setOffered({ error: error.message }),
    );
    return () => {
      current = false;
    };
  }, [api, generation]);
  const options = offered && "options" in offered ? offered.options : null;
  // Read the list again, its entries shown as being read, with no action, until they are: one offered before would
  // act on the text read before the change.
  const reread = () => {
    setEntries({});
    setGeneration((g) => g + 1);
  };
  const confirmDelete = (target: DeleteTarget) => {
    setConfirming(null);
    setFailure(null);
    setDeleting((before) => new Set(before).add(target.name));
    const settled = () => setDeleting((before) => new Set([...before].filter((name) => name !== target.name)));
    api.deleteWorkflow(target.name, target.text).then(
      () => {
        settled();
        setSaved((name) => (name === target.name ? null : name));
        reread();
      },
      (error: Error) => {
        settled();
        setFailure(error.message);
      },
    );
  };
  const onSaved = (name: string) => {
    setEditing(null);
    setSaved(name);
    setFailure(null);
    reread();
  };
  return (
    <>
      <Dialog label="workflows" title="Workflows" className="workflows">
        {offered === null && <p className="loading">Reading the workflows…</p>}
        {offered && "error" in offered && <p className="problem">{offered.error}</p>}
        {options && (
          <ul className="workflow-list">
            {options.workflows.map(({ name }) => (
              <WorkflowItem
                key={name}
                name={name}
                entry={entries[name]}
                saved={name === saved}
                deleting={deleting.has(name)}
                onOpen={setEditing}
                onDelete={setConfirming}
              />
            ))}
          </ul>
        )}
        {failure !== null && <pre className="problem">{failure}</pre>}
        <footer>
          <button type="button" onClick={onClose} disabled={deleting.size > 0}>
            Close
          </button>
          <button type="button" onClick={() => setEditing({ copy: DEFAULT_WORKFLOW })} disabled={!options}>
            New…
          </button>
        </footer>
      </Dialog>
      {confirming && (
        <Dialog label="delete workflow" title={`Delete ${confirming.name}?`} className="confirm">
          <p>
            This removes {confirming.path}. Runs already started with it keep their own copy, and resume with it.
          </p>
          <footer>
            <button type="button" onClick={() => setConfirming(null)}>
              Cancel
            </button>
            <button type="button" onClick={() => confirmDelete(confirming)}>
              Delete
            </button>
          </footer>
        </Dialog>
      )}
      {editing && options && (
        <WorkflowEditor api={api} agents={options.agents} opening={editing} onSaved={onSaved} onClose={() => setEditing(null)} />
      )}
    </>
  );
}

interface ItemProps {
  name: string;
  entry: Entry;
  /** Whether this is the workflow saved last, which the list marks. */
  saved: boolean;
  /** Whether its delete was confirmed and has not been answered. */
  deleting: boolean;
  onOpen: (opening: Opening) => void;
  onDelete: (target: DeleteTarget) => void;
}

/** One workflow of the list: its name, its description or why it does not load, and what can be done with it. */
function WorkflowItem({ name, entry, saved, deleting, onOpen, onDelete }: ItemProps) {
  let body;
  let actions = null;
  if (deleting) {
    body = <p className="loading">Deleting…</p>;
  } else if (entry === undefined) {
    body = <p className="loading">Reading…</p>;
  } else if ("unread" in entry) {
    body = <p className="problem">{entry.unread}</p>;
  } else if ("error" in entry) {
    const { path, text, error } = entry;
    body = (
      <>
        <p className="where">{path}</p>
        <p className="problem">{error}</p>
      </>
    );
    // A file that does not load is fixed by hand, since there is no definition for the form; it can only go.
    actions =
      text === null ? null : (
        <button type="button" onClick={() => onDelete({ name, path, text })}>
          Delete…
        </button>
      );
  } else {
    const { file, definition } = entry;
    body = (
      <>
        {file && <p className="where">{file.path}</p>}
        {definition.description && <p className="description">{definition.description}</p>}
      </>
    );
    actions = (
      <>
        <button type="button" onClick={() => onOpen({ edit: name })}>
          {file ? "Edit…" : "View…"}
        </button>
        <button type="button" onClick={() => onOpen({ copy: name })}>
          Duplicate…
        </button>
        {file && (
          <button type="button" onClick={() => onDelete({ name, path: file.path, text: file.text })}>
            Delete…
          </button>
        )}
      </>
    );
  }
  return (
    <li data-workflow={name} aria-label={`workflow ${name}`} aria-current={saved || undefined}>
      <span className="name">
        {name}
        {entry && "definition" in entry && entry.file === null && " (built in)"}
      </span>
      {body}
      {actions && <div className="actions">{actions}</div>}
    </li>
  );
}
