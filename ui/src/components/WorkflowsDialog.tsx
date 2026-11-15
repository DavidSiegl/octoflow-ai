import { useEffect, useState } from "react";
import type { Api, EditableWorkflow, RunOptions } from "../api";
import { Dialog } from "./Dialog";
import { type Opening, WorkflowEditor } from "./WorkflowEditor";

/** The workflow New copies, which `run` takes without --workflow. */
const NEW_FROM = "default";

/** A workflow of the list, as GET /workflow read it, or the error that kept it from being read; undefined while it
 * is. */
type Entry = EditableWorkflow | { name: string; unread: string } | undefined;

/** A workflow whose file is about to be deleted, once its confirmation is answered: its name, path and text. */
interface Deleting {
  name: string;
  path: string;
  text: string;
}

/** The workflows of this computer, as `run --workflow` offers them, each with its description, or for one whose file
 * does not load, its path and error. New opens the editor on a copy of the default workflow, Duplicate on a copy of
 * the entry, and Edit on the entry itself (View for a built-in one, which is read-only). Delete removes an entry's
 * file after a confirmation naming it. The workflow saved last is marked. */
export function WorkflowsDialog({ api, onClose }: { api: Api; onClose: () => void }) {
  // What /workflows gave, the error that kept it from giving it, or null until it answers.
  const [offered, setOffered] = useState<{ options: RunOptions } | { error: string } | null>(null);
  const [entries, setEntries] = useState<Record<string, Entry>>({});
  // Bumped to read the list again, after a save or a delete.
  const [generation, setGeneration] = useState(0);
  const [editing, setEditing] = useState<Opening | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<Deleting | null>(null);
  // Why the last delete failed; null when it did not.
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
  const confirmDelete = (target: Deleting) => {
    setDeleting(null);
    setFailure(null);
    api.deleteWorkflow(target.name, target.text).then(
      () => {
        setSaved((name) => (name === target.name ? null : name));
        reread();
      },
      (error: Error) => setFailure(error.message),
    );
  };
  const onSaved = (name: string) => {
    setEditing(null);
    setSaved(name);
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
                onOpen={setEditing}
                onDelete={setDeleting}
              />
            ))}
          </ul>
        )}
        {failure !== null && <pre className="problem">{failure}</pre>}
        <footer>
          <button type="button" onClick={onClose}>
            Close
          </button>
          <button type="button" onClick={() => setEditing({ copy: NEW_FROM })} disabled={!options}>
            New…
          </button>
        </footer>
      </Dialog>
      {deleting && (
        <Dialog label="delete workflow" title={`Delete ${deleting.name}?`} className="confirm">
          <p>
            This removes {deleting.path}. Runs already started with it keep their own copy, and resume with it.
          </p>
          <footer>
            <button type="button" onClick={() => setDeleting(null)}>
              Cancel
            </button>
            <button type="button" onClick={() => confirmDelete(deleting)}>
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
  onOpen: (opening: Opening) => void;
  onDelete: (target: Deleting) => void;
}

/** One workflow of the list: its name, its description or why it does not load, and what can be done with it. */
function WorkflowItem({ name, entry, saved, onOpen, onDelete }: ItemProps) {
  let body;
  let actions = null;
  if (entry === undefined) {
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
