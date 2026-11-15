import { useState } from "react";
import type { RunEntry } from "../runs";
import { formatRound, heartbeatAge, outcome, prLabel } from "../runs";
import { StepProgress } from "./StepProgress";

interface Props {
  /** Every run, closed ones included, in the order the table lists them. */
  runs: RunEntry[];
  /** The time, in milliseconds since the epoch, that the heartbeats are aged to. */
  now: number;
  selected: string | null;
  onSelect: (run: string) => void;
}

/** A table of the project's runs, one row per run; a click on a row selects its run. Closed runs are left out until
 * "Show closed runs" is checked, which starts unchecked and is there only while some run is closed. */
export function RunsOverview({ runs, now, selected, onSelect }: Props) {
  const [showClosed, setShowClosed] = useState(false);
  const closed = runs.filter((entry) => entry.record.closed).length;
  const shown = showClosed ? runs : runs.filter((entry) => !entry.record.closed);
  return (
    <>
      {shown.length === 0 && <p className="empty">{closed > 0 ? "No open runs." : "No runs."}</p>}
      {closed > 0 && (
        <label className="show-closed">
          <input type="checkbox" checked={showClosed} onChange={(event) => setShowClosed(event.target.checked)} />
          Show closed runs ({closed})
        </label>
      )}
      {shown.length > 0 && <RunsTable runs={shown} now={now} selected={selected} onSelect={onSelect} />}
    </>
  );
}

function RunsTable({ runs, now, selected, onSelect }: Props) {
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
