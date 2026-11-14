import { describe, expect, it } from "vitest";
import type { AgentRef, RunRecord, Step, StepVerdict, Turn } from "./api";
import { RECORDED } from "./test/fakes";

/** Every key of T, optional ones too. tsc refuses an object of this type that lacks a key of T or has one T lacks,
 * so each list below is T's own keys, not a copy that can fall behind. */
type KeysOf<T> = Record<keyof T, true>;

const RECORD: KeysOf<RunRecord> = {
  id: true, key: true, task: true, workflow: true, steps: true, pull_request: true, phase: true, round: true,
  quality_round: true, role: true, pane: true, agents: true, health: true, health_reason: true, heartbeat_age: true,
  waiting: true, verdict: true, verdicts: true, error: true, closed: true, branch: true, worktree: true, pr: true,
  turns: true, resume_command: true,
};
const STEP: KeysOf<Step> = { id: true, role: true, role_label: true, loop_to: true, quality_gated: true };
const AGENT: KeysOf<AgentRef> = { role: true, name: true, kind: true };
const WAITING: KeysOf<NonNullable<RunRecord["waiting"]>> = { reason: true, detail: true };
const VERDICT: KeysOf<StepVerdict> = { round: true, step: true, verdict: true };
const PR: KeysOf<NonNullable<RunRecord["pr"]>> = { url: true, state: true, error: true };
const TURN: KeysOf<Turn> = {
  step: true, role: true, round: true, quality_round: true, file: true, agent: true, model: true, session: true,
  tokens: true, human_paced: true, started_at: true, ended_at: true, seconds: true,
};

/** The keys api.ts gives a record and each object in it, by where the object is in the record. */
const TYPED: Record<string, Record<string, true>> = {
  record: RECORD, steps: STEP, agents: AGENT, waiting: WAITING, verdicts: VERDICT, pr: PR, turns: TURN,
};

type Json = Record<string, unknown>;

/** The objects the recorded records hold at each place TYPED names: the records themselves, and under a record's
 * key of that name the object there or the items of the list there. */
function recordedObjects(): Record<string, Json[]> {
  const records = RECORDED.flatMap((batch) => batch.events)
    .filter((event) => event.kind === "record" && event.record !== null)
    .map((event) => event.record as Json);
  const found: Record<string, Json[]> = { record: records };
  for (const place of Object.keys(TYPED).filter((place) => place !== "record")) {
    found[place] = records.flatMap((record) => record[place] ?? []) as Json[];
  }
  return found;
}

describe("the keys of a run record", () => {
  it.each(Object.keys(TYPED))("are the same in api.ts as in what serve sent: %s", (place) => {
    const sent = recordedObjects()[place] ?? [];
    // Without an example the recording holds api.ts to nothing here.
    expect(sent.length).toBeGreaterThan(0);
    const typed = Object.keys(TYPED[place] ?? {}).sort();
    for (const object of sent) {
      expect(Object.keys(object).sort()).toEqual(typed);
    }
  });
});
