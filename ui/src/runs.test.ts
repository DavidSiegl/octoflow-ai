import { describe, expect, it } from "vitest";
import type { RunRecord, Step, Turn } from "./api";
import { applyEvent, formatAge, formatRound, heartbeatAge, NO_RUNS, outcome, prLabel, progress } from "./runs";

/** The default workflow's steps as a server since 0.71.0 gives them for a run with the quality gate. */
const STEPS: Step[] = [
  { id: "spec", role: "spec", role_label: "Spec Collector", loop_to: null, quality_gated: false },
  { id: "build", role: "build", role_label: "Builder", loop_to: null, quality_gated: true },
  { id: "review", role: "review", role_label: "Reviewer", loop_to: "build", quality_gated: false },
];

/** The same steps as a server before 0.71.0 gives them. */
const OLD_STEPS: Step[] = STEPS.map(({ id, role }) => ({ id, role }));

function record(overrides: Partial<RunRecord> = {}): RunRecord {
  return {
    id: "20261024-140000-c0ffee", key: "c0ffee", task: "t", workflow: "default", steps: STEPS, pull_request: true,
    phase: "build", round: 1, quality_round: null, role: "build", pane: null, health: "running",
    health_reason: "beat 4s ago", heartbeat_age: 4, waiting: null, verdict: null, error: null, closed: null,
    branch: null, worktree: null, pr: null, turns: [], resume_command: null, ...overrides,
  };
}

const turn = (step: string, round = 1, quality_round = 0): Turn => ({
  step, role: step, round, quality_round, file: `${step}-${round}.md`, agent: step === "quality" ? null : "claude",
  model: null, session: null, tokens: null, human_paced: false, started_at: null, ended_at: null, seconds: null,
});

/** Each stage of the record's progress as [label, state]. */
const states = (r: RunRecord) => progress(r).map((s) => [s.label, s.state]);

describe("progress", () => {
  it("gives every stage from the start, the steps before the phase done", () => {
    expect(states(record())).toEqual([
      ["spec", "done"], ["build", "current"], ["quality", "todo"], ["review", "todo"], ["publish", "todo"],
    ]);
  });

  it("carries each stage's kind, step, role label and loop", () => {
    const stages = progress(record());
    expect(stages.map((s) => [s.kind, s.step, s.roleLabel, s.loopTo])).toEqual([
      ["step", "spec", "Spec Collector", null], ["step", "build", "Builder", null], ["quality", "build", null, null],
      ["step", "review", "Reviewer", "build"], ["publish", null, null, null],
    ]);
  });

  it("gives the quality stage the analyses of its step's turns, and the step its own", () => {
    const turns = [turn("spec"), turn("build"), turn("quality", 1, 1), turn("build", 1, 1), turn("quality", 1, 2)];
    const stages = progress(record({ phase: "quality", quality_round: 2, turns }));
    expect(stages.map((s) => s.turns.length)).toEqual([1, 2, 2, 0, 0]);
    expect(stages[2]?.state).toBe("current");
  });

  it("has quality to do again while its step fixes what the gate found", () => {
    const turns = [turn("spec"), turn("build"), turn("quality", 1, 1)];
    expect(states(record({ phase: "build", quality_round: 1, turns })).slice(1, 3)).toEqual([
      ["build", "current"], ["quality", "again"],
    ]);
  });

  it("puts quality after the step whose turn it judges", () => {
    const steps = STEPS.map((s) => ({ ...s, quality_gated: s.id !== "spec" }));
    const turns = [turn("spec"), turn("build"), turn("quality", 1, 1), turn("review"), turn("quality", 1, 1)];
    expect(states(record({ steps, phase: "quality", turns }))).toEqual([
      ["spec", "done"], ["build", "done"], ["quality", "done"], ["review", "done"], ["quality", "current"],
      ["publish", "todo"],
    ]);
  });

  it("is in the quality stage of the step it just left, before the gate's first analysis is saved", () => {
    const steps = STEPS.map((s) => ({ ...s, quality_gated: s.id !== "spec" }));
    const turns = [turn("spec"), turn("build"), turn("quality", 1, 1), turn("review")];
    expect(states(record({ steps, phase: "quality", turns })).at(-2)).toEqual(["quality", "current"]);
  });

  it("marks a stage that ran before the run looped back", () => {
    const turns = [turn("spec"), turn("build"), turn("quality", 1, 1), turn("review"), turn("build", 2)];
    expect(states(record({ round: 2, turns }))).toEqual([
      ["spec", "done"], ["build", "current"], ["quality", "again"], ["review", "again"], ["publish", "todo"],
    ]);
  });

  it("ends in publish only for a run that opens a pull request", () => {
    expect(states(record({ phase: "publish" })).map(([, state]) => state)).toEqual(["done", "done", "done", "done", "current"]);
    expect(states(record({ pull_request: false })).map(([label]) => label)).toEqual(["spec", "build", "quality", "review"]);
  });

  it("marks every stage done once the run is", () => {
    expect(states(record({ phase: "done" })).every(([, state]) => state === "done")).toBe(true);
  });

  it("marks a failed run's stage failed", () => {
    expect(states(record({ error: "boom" }))[1]).toEqual(["build", "failed"]);
  });

  it("shows a record from a server before 0.71.0 as far as it says", () => {
    const old = (overrides: Partial<RunRecord>) => {
      const r = record({ steps: OLD_STEPS, ...overrides });
      delete r.pull_request;
      return r;
    };
    expect(progress(old({})).map((s) => [s.label, s.state, s.roleLabel, s.loopTo])).toEqual([
      ["spec", "done", "spec", null], ["build", "current", "build", null], ["review", "todo", "review", null],
    ]);
    expect(states(old({ phase: "quality", turns: [turn("spec"), turn("build")] }))).toEqual([
      ["spec", "done"], ["build", "done"], ["quality", "current"], ["review", "todo"],
    ]);
    expect(states(old({ phase: "publish" }))).toEqual([
      ["spec", "done"], ["build", "done"], ["review", "done"], ["publish", "current"],
    ]);
  });

  it("shows only the phase of a workflow that could not be loaded", () => {
    expect(states(record({ steps: null, phase: "impl" }))).toEqual([["impl", "current"]]);
  });
});

describe("wording", () => {
  it("formats ages as list does", () => {
    expect([0, 59, 60, 3599, 3600, 7380].map(formatAge)).toEqual(["0s", "59s", "1m", "59m", "1h0m", "2h3m"]);
  });

  it("formats the round with its quality round", () => {
    expect(formatRound(record())).toBe("round 1");
    expect(formatRound(record({ quality_round: 2 }))).toBe("round 1 q2");
  });

  it("shortens a GitHub pull request to its number", () => {
    expect(prLabel("https://github.com/o/r/pull/53")).toBe("#53");
    expect(prLabel("https://gitlab.example/o/r/-/merge_requests/7")).toBe("https://gitlab.example/o/r/-/merge_requests/7");
  });

  it("words the outcome", () => {
    expect(outcome(record(), 12)).toBe("running, beat 12s ago");
    expect(outcome(record({ health: "stale" }), 400)).toBe("stale, beat 6m ago");
    expect(outcome(record({ health: null, verdict: "APPROVE" }), 0)).toBe("APPROVE");
    expect(outcome(record({ error: "boom" }), 0)).toBe("error: boom");
  });
});

describe("applyEvent", () => {
  it("keeps a run's files across a new record, and ages from the record's arrival", () => {
    let state = applyEvent(NO_RUNS, { kind: "record", run: "r", record: record() }, 1000);
    state = applyEvent(state, { kind: "files", run: "r", files: [{ name: "spec.md", mtime: 1 }] }, 1000);
    state = applyEvent(state, { kind: "record", run: "r", record: record({ heartbeat_age: 0 }) }, 5000);
    const entry = state.runs.r;
    expect(entry?.files).toEqual([{ name: "spec.md", mtime: 1 }]);
    expect(entry && heartbeatAge(entry, 7900)).toBe(2);
  });

  it("ignores files of a run it does not know", () => {
    expect(applyEvent(NO_RUNS, { kind: "files", run: "gone", files: [] }, 0)).toBe(NO_RUNS);
  });
});
