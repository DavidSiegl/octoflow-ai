import { useState } from "react";
import type { Api, Project, RunRecord } from "../api";
import { useNow } from "../hooks";
import type { RunEntry } from "../runs";
import { formatRound, heartbeatAge, outcome } from "../runs";
import { AgentPanes } from "./AgentPanes";
import { HandoffFile } from "./HandoffFile";
import { OutsideLink } from "./OutsideLink";
import { RunMap } from "./RunMap";

interface Props {
  api: Api;
  project: Project;
  entry: RunEntry;
  onClose: () => void;
}

/** One run: its map, its files, and the selected one rendered, the newest file until one is chosen; and its
 * agents. */
export function RunDetail({ api, project, entry, onClose }: Props) {
  const { record, files } = entry;
  const [chosen, setChosen] = useState<string | null>(null);
  const file = files?.find((f) => f.name === chosen) ?? files?.at(-1);
  return (
    <section className="detail" aria-label={`run ${record.key}`}>
      <header>
        <button type="button" onClick={onClose}>
          ‹ Runs
        </button>
        <h2 title={record.id}>{record.key}</h2>
        <p className="task">{record.task}</p>
        <Status entry={entry} />
        {record.waiting?.reason === "blocked" && <p className="blocked">blocked: {record.waiting.detail}</p>}
        {record.resumable && record.backend === "acp" && <ResumeButton api={api} project={project} record={record} />}
        {record.pr && (
          <OutsideLink target={{ page: record.pr.url }}>{record.pr.url}</OutsideLink>
        )}
        {/* Worded as the engine's summary.md and show word it (NOTHING_TO_PUBLISH in engine/src/state.rs). */}
        {!record.pr && record.nothing_to_publish && (
          <p className="no-pr">No pull request: nothing tracked changed since the base, so there was nothing to publish.</p>
        )}
      </header>
      <RunMap record={record} />
      <div className="files-and-file">
        <ul className="files" aria-label="files">
          {(files ?? []).map((f) => (
            <li key={f.name}>
              <button
                type="button"
                className={f.name === file?.name ? "selected" : undefined}
                onClick={() => setChosen(f.name)}
              >
                {f.name}
                <time dateTime={new Date(f.mtime * 1000).toISOString()}>
                  {new Date(f.mtime * 1000).toLocaleTimeString()}
                </time>
              </button>
            </li>
          ))}
        </ul>
        {file ? (
          <HandoffFile api={api} project={project} run={record.id} file={file} />
        ) : (
          <p className="empty">{files ? "No files yet." : "Loading…"}</p>
        )}
      </div>
      <AgentPanes api={api} project={project} record={record} />
    </section>
  );
}

/** The run's round, role and outcome, its heartbeat aged to the clock. The rest of the run detail changes only with
 * the run, so the clock's tick re-renders this line alone, not the octopus, the handoff file or the agents. */
function Status({ entry }: { entry: RunEntry }) {
  const now = useNow();
  const { record } = entry;
  return (
    <p className="status">
      {formatRound(record)}
      {record.role && ` · ${record.role}`} · {outcome(record, heartbeatAge(entry, now))}
    </p>
  );
}

/** Resume, for a desktop app's run a resume would take up (RunRecord.resumable), and why the last resume failed. The
 * run's next record, from the event stream, shows it resumed. From the click until that record comes, or the resume
 * fails, Resume cannot be clicked again: a second resume would end the first one's agents (POST /resume). */
function ResumeButton({ api, project, record }: { api: Api; project: Project; record: RunRecord }) {
  // The record Resume was clicked on, until the resume fails; null before a click. The stream sends a run's record
  // only when it changes, so while this is still the record shown, the resume has not shown in it yet.
  const [clickedOn, setClickedOn] = useState<RunRecord | null>(null);
  // True when the last resume started but had not taken the run up by the end of the server's wait.
  const [unlisted, setUnlisted] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const resuming = clickedOn === record;
  const resume = () => {
    setClickedOn(record);
    setUnlisted(false);
    setProblem(null);
    api.resumeRun(project, record.id).then(
      (listed) => setUnlisted(!listed),
      (e: unknown) => {
        setClickedOn(null);
        setProblem(e instanceof Error ? e.message : String(e));
      },
    );
  };
  return (
    <p className="resume">
      <button type="button" onClick={resume} disabled={resuming}>
        {resuming ? "Resuming…" : "Resume"}
      </button>
      {resuming && unlisted && <span className="note"> The resume has started, but the run does not show it yet.</span>}
      {problem && <span className="problem"> {problem}</span>}
    </p>
  );
}
