import { useEffect, useState } from "react";
import type { Api, ClosedRun, Project } from "../api";
import type { RunEntry } from "../runs";
import { Dialog } from "./Dialog";

interface Props {
  api: Api;
  project: Project;
  /** The project's runs as the overview has them, by id, for each run's task. */
  runs: Record<string, RunEntry>;
  onClose: () => void;
}

/** What a sweep gave: each run it looked at, the error that kept the server from sweeping, or null while it runs. */
type Sweep = { runs: ClosedRun[] } | { error: string } | null;

/** The outcomes listed run by run; a run already closed is only counted. */
type Listed = Exclude<ClosedRun["outcome"], "already closed">;

/** How a listed run's outcome is worded before closing (a dry run's) and after, and its state for `.checks`' colours. */
const WORDS: Record<Listed, { before: string; after: string; state: string }> = {
  closed: { before: "close", after: "closed", state: "ok" },
  skipped: { before: "skip", after: "skipped", state: "warn" },
  failed: { before: "failed", after: "failed", state: "missing" },
};

/** Clean up the project's runs as `close --all` does: first what closing each would do, as a dry run says, then,
 * on Close, what closing the runs it showed did (Api.closeRuns). A run closed before is only counted. With nothing
 * to close it offers no Close, and says why: nothing is left, or runs could not be looked at. The overview follows
 * the closed runs through its own event stream. */
export function CleanUpDialog({ api, project, runs, onClose }: Props) {
  const [preview, setPreview] = useState<Sweep>(null);
  // The real sweep: undefined until Close is clicked, then as Sweep says.
  const [closed, setClosed] = useState<Sweep | undefined>(undefined);
  useEffect(() => {
    let current = true;
    api.previewClose(project).then(
      (swept) => current && setPreview({ runs: swept }),
      (error: Error) => current && setPreview({ error: error.message }),
    );
    return () => {
      current = false;
    };
  }, [api, project]);
  const previewed = preview && "runs" in preview ? preview.runs : null;
  const close = () => {
    if (previewed === null) {
      return;
    }
    setClosed(null);
    api.closeRuns(project, previewed).then(
      (swept) => setClosed({ runs: swept }),
      (error: Error) => setClosed({ error: error.message }),
    );
  };
  const toClose = previewed?.filter((r) => r.outcome === "closed").length ?? 0;
  const unread = previewed?.filter((r) => r.outcome === "failed").length ?? 0;
  return (
    <Dialog label="clean up runs" title="Clean up runs" project={project} className="clean-up">
      {closed === undefined ? (
        <SweepList sweep={preview} runs={runs} previewing={true} waiting="Looking at the runs…" />
      ) : (
        <SweepList sweep={closed} runs={runs} previewing={false} waiting="Closing…" />
      )}
      {closed === undefined && previewed !== null && (
        unread > 0 ? (
          <p className="problem">
            {unread === 1 ? "1 run" : `${unread} runs`} could not be looked at, so {unread === 1 ? "it stays" : "they stay"} open.
          </p>
        ) : (
          toClose === 0 && <p>Nothing to close.</p>
        )
      )}
      <footer>
        {closed === undefined ? (
          <>
            <button type="button" onClick={onClose}>
              Cancel
            </button>
            {toClose > 0 && (
              <button type="button" onClick={close}>
                Close {toClose === 1 ? "1 run" : `${toClose} runs`}
              </button>
            )}
          </>
        ) : (
          <button type="button" onClick={onClose} disabled={closed === null}>
            Done
          </button>
        )}
      </footer>
    </Dialog>
  );
}

/** The runs of a sweep, each with its task, outcome as WORDS has it before closing (previewing, for a dry run) or
 * after, and detail, and the uncommitted changes that go with its worktree; the runs already closed as a count; the
 * sweep's error; or waiting while it runs. */
function SweepList({ sweep, runs, previewing, waiting }: { sweep: Sweep; runs: Record<string, RunEntry>; previewing: boolean; waiting: string }) {
  if (sweep === null) {
    return <p className="loading">{waiting}</p>;
  }
  if ("error" in sweep) {
    return <p className="problem">{sweep.error}</p>;
  }
  const listed = sweep.runs.filter((r): r is ClosedRun & { outcome: Listed } => r.outcome !== "already closed");
  const already = sweep.runs.length - listed.length;
  return (
    <>
      <ul className="checks">
        {listed.map((r) => {
          const words = WORDS[r.outcome];
          return (
            <li key={r.run} data-run={r.run} data-state={words.state}>
              <span className="word">{previewing ? words.before : words.after}</span>{" "}
              <span className="name">{runs[r.run]?.record.task ?? r.run}</span>: {r.detail}
              {r.discarded.length > 0 && (
                <>
                  <p className="fix">
                    {previewing ? "Uncommitted changes that go with the worktree:" : "Uncommitted changes discarded with the worktree:"}
                  </p>
                  <pre className="discarded">{r.discarded.join("\n")}</pre>
                </>
              )}
            </li>
          );
        })}
      </ul>
      {already > 0 && <p className="loading">{already === 1 ? "1 run was" : `${already} runs were`} closed before.</p>}
    </>
  );
}
