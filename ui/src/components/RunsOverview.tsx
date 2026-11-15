import type { RunEntry } from "../runs";
import { formatRound, heartbeatAge, outcome, prLabel } from "../runs";
import { StepProgress } from "./StepProgress";

interface TableProps {
  /** The runs to list, in the order the table lists them. */
  runs: RunEntry[];
  /** The time, in milliseconds since the epoch, that the heartbeats are aged to. */
  now: number;
  selected: string | null;
  onSelect: (run: string) => void;
}

interface Props extends TableProps {
  /** Every run, closed ones included. */
  runs: RunEntry[];
  /** Whether closed runs are listed: the "Show closed runs" checkbox, which onShowClosed reports changes of. */
  showClosed: boolean;
  onShowClosed: (show: boolean) => void;
  /** Called on a click of "Delete closed runs…". */
  onForget: () => void;
}

/** A table of the project's runs, one row per run; a click on a row selects its run. Closed runs are listed only
 * with showClosed; the checkbox that sets it, and the button that deletes them, are there only while some run is
 * closed. */
export function RunsOverview({ runs, now, selected, onSelect, showClosed, onShowClosed, onForget }: Props) {
  const closedCount = runs.filter((entry) => entry.record.closed).length;
  const shown = showClosed ? runs : runs.filter((entry) => !entry.record.closed);
  return (
    <>
      {shown.length === 0 && <p className="empty">{closedCount > 0 ? "No open runs." : "No runs."}</p>}
      {closedCount > 0 && (
        <div className="closed-runs">
          <label>
            <input type="checkbox" checked={showClosed} onChange={(event) => onShowClosed(event.target.checked)} />
            Show closed runs ({closedCount})
          </label>
          <button type="button" onClick={onForget}>
            Delete closed runs…
          </button>
        </div>
      )}
      {shown.length > 0 && <RunsTable runs={shown} now={now} selected={selected} onSelect={onSelect} />}
    </>
  );
}

function RunsTable({ runs, now, selected, onSelect }: TableProps) {
  return (
    <table className="runs">
      <thead>
        <tr>
          <th>Run</th>
          <th>Task</th>
          <th>Progress</th>
          <th>Round</th>
          <th>Role</th>
          <th>Status</th>
          <th>Pull request</th>
        </tr>
      </thead>
      <tbody>
        {runs.map((entry) => {
          const { record } = entry;
          return (
            <tr
              key={record.id}
              data-run={record.id}
              className={record.id === selected ? "selected" : undefined}
              onClick={() => onSelect(record.id)}
            >
              <td className="key" title={record.id}>
                {record.key}
              </td>
              <td className="task" title={record.task}>
                {record.task}
              </td>
              <td>
                <StepProgress record={record} />
              </td>
              <td>{formatRound(record)}</td>
              <td className="role">{record.role ?? ""}</td>
              <td className={`status ${record.error ? "failed" : (record.health ?? "finished")}`}>
                {outcome(record, heartbeatAge(entry, now))}
                {record.waiting?.reason === "blocked" && <div className="blocked">blocked: {record.waiting.detail}</div>}
              </td>
              <td>
                {record.pr && (
                  <a href={record.pr.url} title={record.pr.url} target="_blank" rel="noreferrer">
                    {prLabel(record.pr.url)}
                  </a>
                )}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
