import { useEffect, useState } from "react";
import type { Api, ForgottenRun, Project } from "../api";
import type { RunEntry } from "../runs";
import { Dialog } from "./Dialog";

interface Props {
  api: Api;
  project: Project;
  /** The project's runs as the overview has them when the dialog opens, by id, for each run's task; read only then,
   * as the runs the dialog deletes leave the stream while it still shows them. */
  runs: Record<string, RunEntry>;
  onClose: () => void;
}

/** What a sweep gave: each run it looked at, the error that kept the server from sweeping, or null while it runs. */
type Sweep = { runs: ForgottenRun[] } | { error: string } | null;

/** The outcomes listed run by run; a run waiting for prune is only counted, and an open one left out. */
type Listed = Exclude<ForgottenRun["outcome"], "not closed" | "waiting for prune">;

/** How a listed run's outcome is worded before deleting (a dry run's) and after, and its state for `.checks`' colours. */
const WORDS: Record<Listed, { before: string; after: string; state: string }> = {
  forgotten: { before: "delete", after: "deleted", state: "ok" },
  skipped: { before: "skip", after: "skipped", state: "warn" },
  failed: { before: "failed", after: "failed", state: "missing" },
};

/** Delete the project's closed runs as `forget --all` does: first which runs that would delete, as a dry run says,
 * then, on Delete, how deleting each went (Api.forgetRuns). With nothing to delete it offers no Delete. The overview
 * follows the deleted runs through its own event stream. */
export function ForgetDialog({ api, project, runs, onClose }: Props) {
  const [tasks] = useState(() => new Map(Object.entries(runs).map(([id, entry]) => [id, entry.record.task])));
  const [preview, setPreview] = useState<Sweep>(null);
  // The real sweep: undefined until Delete is clicked, then as Sweep says.
  const [forgotten, setForgotten] = useState<Sweep | undefined>(undefined);
  useEffect(() => {
    let current = true;
    api.previewForget(project).then(
      (swept) => current && setPreview({ runs: swept }),
      (error: Error) => current && setPreview({ error: error.message }),
    );
    return () => {
      current = false;
    };
  }, [api, project]);
  const previewed = preview && "runs" in preview ? preview.runs : null;
  const forget = () => {
    if (previewed === null) {
      return;
    }
    setForgotten(null);
    api.forgetRuns(project, previewed).then(
      (swept) => setForgotten({ runs: swept }),
      (error: Error) => setForgotten({ error: error.message }),
    );
  };
  const toDelete = previewed?.filter((r) => r.outcome === "forgotten").length ?? 0;
  return (
    <Dialog label="delete closed runs" title="Delete closed runs" project={project} className="forget">
      {forgotten === undefined ? (
        <SweepList sweep={preview} tasks={tasks} previewing={true} waiting="Looking at the runs…" />
      ) : (
        <SweepList sweep={forgotten} tasks={tasks} previewing={false} waiting="Deleting…" />
      )}
      {forgotten === undefined && previewed !== null && toDelete === 0 && <p>Nothing to delete.</p>}
      <footer>
        {forgotten === undefined ? (
          <>
            <button type="button" onClick={onClose}>
              Cancel
            </button>
            {toDelete > 0 && (
              <button type="button" onClick={forget}>
                Delete {toDelete === 1 ? "1 run" : `${toDelete} runs`}
              </button>
            )}
          </>
        ) : (
          <button type="button" onClick={onClose} disabled={forgotten === null}>
            Done
          </button>
        )}
      </footer>
    </Dialog>
  );
}

/** The closed runs of a sweep, each with its task as tasks has it by run id, outcome as WORDS has it before deleting
 * (previewing, for a dry run) or after, and detail; the runs waiting for prune as a count; the sweep's error; or
 * waiting while it runs. */
function SweepList({ sweep, tasks, previewing, waiting }: { sweep: Sweep; tasks: Map<string, string>; previewing: boolean; waiting: string }) {
  if (sweep === null) {
    return <p className="loading">{waiting}</p>;
  }
  if ("error" in sweep) {
    return <p className="problem">{sweep.error}</p>;
  }
  const listed = sweep.runs.filter((r): r is ForgottenRun & { outcome: Listed } => r.outcome !== "not closed" && r.outcome !== "waiting for prune");
  const pruned = sweep.runs.filter((r) => r.outcome === "waiting for prune").length;
  return (
    <>
      <ul className="checks">
        {listed.map((r) => {
          const words = WORDS[r.outcome];
          return (
            <li key={r.run} data-run={r.run} data-state={words.state}>
              <span className="word">{previewing ? words.before : words.after}</span>{" "}
              <span className="name">{tasks.get(r.run) ?? r.run}</span>
              {r.detail && `: ${r.detail}`}
            </li>
          );
        })}
      </ul>
      {pruned > 0 && (
        <p className="loading">
          {pruned === 1 ? "1 run waits" : `${pruned} runs wait`} for prune to delete {pruned === 1 ? "its branch" : "their branches"} first.
        </p>
      )}
    </>
  );
}
