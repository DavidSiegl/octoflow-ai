import type { RunEvent, RunFile, RunRecord, Turn } from "./api";

/** A run as the app last heard of it. */
export interface RunEntry {
  record: RunRecord;
  /** When the record arrived, in milliseconds since the epoch: its heartbeat_age was the age then. */
  receivedAt: number;
  /** null until the run's first files event. */
  files: RunFile[] | null;
}

/** What a project's event stream has told the app. */
export interface RunsState {
  runs: Record<string, RunEntry>;
  /** Why the server could not read the project, as its last problem event said; null once it can. */
  problem: string | null;
}

export const NO_RUNS: RunsState = { runs: {}, problem: null };

/** The state after an event that arrived at the time `at`, in milliseconds since the epoch. */
export function applyEvent(state: RunsState, event: RunEvent, at: number): RunsState {
  switch (event.kind) {
    case "problem":
      return { ...state, problem: event.error };
    case "record": {
      const { [event.run]: old, ...others } = state.runs;
      if (event.record === null) {
        return { ...state, runs: others };
      }
      const entry = { record: event.record, receivedAt: at, files: old?.files ?? null };
      return { ...state, runs: { ...others, [event.run]: entry } };
    }
    case "files": {
      const old = state.runs[event.run];
      // The server sends a run's record before its files, so a run unknown here has gone since.
      return old ? { ...state, runs: { ...state.runs, [event.run]: { ...old, files: event.files } } } : state;
    }
  }
}

/** The runs, newest first: run ids start with their start time. */
export function runsNewestFirst(state: RunsState): RunEntry[] {
  return Object.values(state.runs).sort((a, b) => b.record.id.localeCompare(a.record.id));
}

/** The seconds since the run's state.json was written, at the time now in milliseconds since the epoch. */
export function heartbeatAge(entry: RunEntry, now: number): number {
  return entry.record.heartbeat_age + Math.max(0, Math.floor((now - entry.receivedAt) / 1000));
}

/** seconds as `list` words an age (orchestrator.py's format_age): 42s, 5m, 2h3m. */
export function formatAge(seconds: number): string {
  if (seconds < 60) {
    return `${seconds}s`;
  }
  if (seconds < 3600) {
    return `${Math.floor(seconds / 60)}m`;
  }
  return `${Math.floor(seconds / 3600)}h${Math.floor((seconds % 3600) / 60)}m`;
}

/** A pull request's URL as the overview shows it: #N for a GitHub pull request, else the whole URL. */
export function prLabel(url: string): string {
  const number = url.match(/\/pull\/(\d+)$/)?.[1];
  return number ? `#${number}` : url;
}

/** The round as `list` words it (run_round): "round 2", with " q1" in a quality round. */
export function formatRound(record: RunRecord): string {
  return record.quality_round ? `round ${record.round} q${record.quality_round}` : `round ${record.round}`;
}

/** A stage's place in the run: done; current, or failed when the run failed there; to do; or "again", a stage after
 * the current one that already ran: a verdict sent the run back to an earlier step, or the quality gate sent it back
 * to its step for a fix in the same round. */
export type StageState = "done" | "current" | "todo" | "failed" | "again";

/** One stage of a run's progress, in the order progress gives them. */
export interface Stage {
  /** "step", a step of the workflow; "quality", the quality loop after a quality-gated step; "publish", the pull
   * request; "phase", the phase alone, of a run whose workflow the server could not load. */
  kind: "step" | "quality" | "publish" | "phase";
  /** The step's id; for quality, the id of the gated step its loop judges; null for publish and phase. */
  step: string | null;
  /** The stage's name as the run words it: the step id, "quality", "publish", or the phase. */
  label: string;
  /** The workflow's label for the step's role, the role itself from a server before 0.71.0; null but for a step. */
  roleLabel: string | null;
  /** The step a CHANGES_REQUESTED from this step sends the run back to; null for a step that gives no verdict, and
   * for every stage of a record from a server before 0.71.0. */
  loopTo: string | null;
  /** The stage's turns, oldest first: a step's own turns, and for quality, the gate's analyses of that step's
   * turns. Empty for publish and phase. */
  turns: Turn[];
  /** Where the run stands with the stage. */
  state: StageState;
}

/** The run's way through its workflow, every stage of it from the start: each step, quality after each step the
 * quality loop follows in this run, and publish last when the run opens a pull request.
 *
 * The current stage is the phase's step; in quality, the quality stage after the step whose turn it judges, the last
 * one taken; in publish, publish. A failed run's current stage is "failed". A record from a server before 0.71.0 does
 * not say which steps are quality-gated or whether the run publishes, so its quality and publish stages appear only
 * while the run is in them. A workflow the server could not load has no steps, and its progress is its phase alone.
 */
export function progress(record: RunRecord): Stage[] {
  const current: StageState = record.error ? "failed" : "current";
  if (!record.steps) {
    return [{ kind: "phase", step: null, label: record.phase, roleLabel: null, loopTo: null, turns: [], state: current }];
  }
  const ids = record.steps.map((step) => step.id);
  // A quality analysis is saved under the phase, quality; it judges the step whose turn came last before it.
  const judged = new Map<Turn, string>();
  let lastStep = ids.at(-1) ?? "";
  for (const turn of record.turns) {
    if (ids.includes(turn.step)) {
      lastStep = turn.step;
    } else if (turn.step === "quality") {
      judged.set(turn, lastStep);
    }
  }
  const inQualityAfter = record.phase === "quality" ? lastStep : null;
  const stages: Omit<Stage, "state">[] = [];
  for (const step of record.steps) {
    stages.push({
      kind: "step", step: step.id, label: step.id, roleLabel: step.role_label ?? step.role, loopTo: step.loop_to ?? null,
      turns: record.turns.filter((turn) => turn.step === step.id),
    });
    if (step.quality_gated || inQualityAfter === step.id) {
      const turns = record.turns.filter((turn) => judged.get(turn) === step.id);
      stages.push({ kind: "quality", step: step.id, label: "quality", roleLabel: null, loopTo: null, turns });
    }
  }
  if (record.pull_request || record.phase === "publish") {
    stages.push({ kind: "publish", step: null, label: "publish", roleLabel: null, loopTo: null, turns: [] });
  }
  const at =
    record.phase === "done" ? stages.length
    : record.phase === "quality" ? stages.findIndex((st) => st.kind === "quality" && st.step === inQualityAfter)
    : record.phase === "publish" ? stages.length - 1
    : stages.findIndex((st) => st.kind === "step" && st.step === record.phase);
  return stages.map((stage, i): Stage => {
    const after: StageState = stage.turns.length > 0 ? "again" : "todo";
    return { ...stage, state: at === -1 ? "todo" : i < at ? "done" : i === at ? current : after };
  });
}

/** How the run is doing, as the overview words it: its error, verdict, block, or health with a ticking beat. */
export function outcome(record: RunRecord, age: number): string {
  if (record.error) {
    return `error: ${record.error}`;
  }
  if (record.health === null) {
    return record.verdict ?? "";
  }
  return `${record.health}, beat ${formatAge(age)} ago`;
}
