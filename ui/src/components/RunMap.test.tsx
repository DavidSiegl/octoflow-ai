import { act, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { RunRecord, Step, Turn } from "../api";
import { RunMap } from "./RunMap";

const NOW = Date.parse("2026-11-13T14:30:00Z");
const ago = (seconds: number) => new Date(NOW - seconds * 1000).toISOString();

const STEPS: Step[] = [
  { id: "spec", role: "spec", role_label: "Spec Collector", loop_to: null, quality_gated: false },
  { id: "build", role: "build", role_label: "Builder", loop_to: null, quality_gated: true },
  { id: "review", role: "review", role_label: "Reviewer", loop_to: "build", quality_gated: false },
];

/** A turn of step in round, started `start` seconds before NOW and ended `end` seconds before it, or still running
 * with end null. */
function turn(step: string, round: number, file: string, start: number, end: number | null, extra: Partial<Turn> = {}): Turn {
  const quality = step === "quality";
  return {
    step, role: step, round, quality_round: 0, file, agent: quality ? null : "claude", model: quality ? null : "opus-5.5",
    session: null, tokens: null, human_paced: false, started_at: ago(start), ended_at: end === null ? null : ago(end),
    seconds: end === null ? null : start - end, ...extra,
  };
}

/** A default-workflow run back in build after a review asked for changes. */
function record(overrides: Partial<RunRecord> = {}): RunRecord {
  return {
    id: "20261113-130000-fea142", key: "fea142", task: "t", workflow: "default", steps: STEPS, pull_request: true,
    phase: "build", round: 2, quality_round: null, role: "build", pane: null, health: "running", health_reason: null,
    heartbeat_age: 3, waiting: null, verdict: null, error: null, closed: null, branch: null, worktree: null, pr: null,
    verdicts: [{ round: 1, step: "review", verdict: "CHANGES_REQUESTED" }],
    turns: [
      turn("spec", 0, "spec.md", 3900, 3480, { human_paced: true }),
      turn("build", 1, "build-1.md", 3470, 2010),
      turn("quality", 1, "quality-1.md", 2000, 1880, { quality_round: 1 }),
      turn("review", 1, "review-1.md", 1870, 1150, { agent: "codex", model: "gpt-5.3" }),
      turn("build", 2, "build-2.md", 1140, null),
    ],
    resume_command: null, ...overrides,
  };
}

function stations(): HTMLElement[] {
  return within(screen.getByRole("list", { name: "stages" })).getAllByRole("listitem");
}

describe("RunMap", () => {
  it("gives each stage a station in order, named with who holds it and its state", () => {
    render(<RunMap record={record()} now={NOW} />);
    expect(stations().map((s) => s.getAttribute("aria-label"))).toEqual([
      "spec, Spec Collector, claude opus-5.5, done",
      "build, Builder, claude opus-5.5, working, 2 rounds",
      "quality, quality gate, ran before, to do again",
      "review, Reviewer, codex gpt-5.3, ran before, to do again, last verdict CHANGES_REQUESTED, loops to build",
      "publish, pull request, to do",
    ]);
  });

  it("shows the harness and model of each stage's last turn, a rounds badge and the last verdict", () => {
    render(<RunMap record={record()} now={NOW} />);
    const [spec, build, , review] = stations();
    expect(spec && within(spec).getByText("claude opus-5.5")).toBeInTheDocument();
    expect(build && within(build).getByText("×2")).toBeInTheDocument();
    expect(review && within(review).getByText("codex gpt-5.3")).toBeInTheDocument();
    expect(review && within(review).getByText("changes")).toBeInTheDocument();
    expect(spec && within(spec).queryByText(/×/)).not.toBeInTheDocument();
  });

  it("cuts a harness too wide for the room between neighbouring stations, and names it whole", () => {
    const steps: Step[] = ["a", "b", "c", "d", "e", "f", "g", "h"].map((id) => ({
      id, role: id, role_label: id.toUpperCase(), loop_to: null, quality_gated: false,
    }));
    const turns = steps.map((st, i) => turn(st.id, 1, `${st.id}.md`, 900 - i * 100, 850 - i * 100, { model: "claude-opus-4-5-20251101" }));
    render(<RunMap record={record({ steps, pull_request: false, phase: "done", health: null, turns })} now={NOW} />);
    // d's station has e's beside it, about 106 units away; a's, at the left edge, has 180 units of room.
    const [a, , , d] = stations();
    expect(d?.querySelector(".harness")?.textContent).toMatch(/^claude .*…$/);
    expect(d).toHaveAccessibleName("d, D, claude claude-opus-4-5-20251101, done");
    expect(a?.querySelector(".harness")?.textContent).toMatch(/^claude .*…$/);
    expect(a?.querySelector(".holder")?.textContent).toBe("A");
  });

  it("draws a verdict step's loop, solid once a verdict took it", () => {
    const { container, rerender } = render(<RunMap record={record()} now={NOW} />);
    expect(screen.getByText("changes → build")).toBeInTheDocument();
    expect(container.querySelector(".loop.taken")).not.toBeNull();
    rerender(<RunMap record={record({ verdicts: [], phase: "spec", round: 0, turns: [] })} now={NOW} />);
    expect(screen.getByText("loops to build")).toBeInTheDocument();
    expect(container.querySelector(".loop.taken")).toBeNull();
  });

  it("names the pull request at publish", () => {
    const pr = { url: "https://github.com/o/r/pull/408", state: "OPEN", error: null };
    render(<RunMap record={record({ phase: "done", health: null, verdict: "APPROVE", pr })} now={NOW} />);
    expect(stations().at(-1)).toHaveAccessibleName("publish, #408, done");
    expect(screen.getByText("done · APPROVE · pull request open")).toBeInTheDocument();
  });

  it("asks the human when the run waits on them", () => {
    const { rerender } = render(<RunMap record={record({ waiting: { reason: "interview", detail: "spec" } })} now={NOW} />);
    expect(screen.getByText("your answer?")).toBeInTheDocument();
    rerender(<RunMap record={record({ waiting: { reason: "blocked", detail: "which key?" } })} now={NOW} />);
    expect(screen.getByText("blocked")).toBeInTheDocument();
    rerender(<RunMap record={record({ waiting: { reason: "stale", detail: "orchestrator resume fea142" } })} now={NOW} />);
    expect(screen.queryByText("blocked")).not.toBeInTheDocument();
  });

  it("says what runs now, then lists the turns newest first", () => {
    render(<RunMap record={record()} now={NOW} />);
    expect(screen.getByText("build · claude opus-5.5 · running 19m")).toBeInTheDocument();
    const rows = within(screen.getByRole("list", { name: "turns" })).getAllByRole("listitem");
    expect(rows.map((row) => [...row.children].map((cell) => cell.textContent))).toEqual([
      ["●", "build", "build-2.md · claude opus-5.5", "running"],
      ["·", "review", "review-1.md · codex gpt-5.3", "12m"],
      ["·", "quality q1", "quality-1.md · quality gate", "2m"],
      ["·", "build", "build-1.md · claude opus-5.5", "24m"],
      ["·", "spec", "spec.md · claude opus-5.5", "7m"],
    ]);
  });

  it("marks where a failed run stopped", () => {
    const { container } = render(<RunMap record={record({ error: "boom", health: null })} now={NOW} />);
    expect(screen.getByText("failed at build")).toBeInTheDocument();
    expect(stations()[1]).toHaveAttribute("data-state", "failed");
    expect(container.querySelector(".head.failed")).not.toBeNull();
  });

  it("marks the turn the run failed in, not an earlier step's", () => {
    const ended = record().turns.slice(0, 4);
    render(<RunMap record={record({ phase: "publish", error: "push refused", health: null, turns: ended })} now={NOW} />);
    const rows = within(screen.getByRole("list", { name: "turns" })).getAllByRole("listitem");
    expect(rows.map((row) => row.firstChild?.textContent)).toEqual(["·", "·", "·", "·"]);
  });

  it("marks a failed run's turn that never ended as failed, not running", () => {
    render(<RunMap record={record({ error: "boom", health: null })} now={NOW} />);
    const [newest] = within(screen.getByRole("list", { name: "turns" })).getAllByRole("listitem");
    expect([...(newest?.children ?? [])].map((cell) => cell.textContent)).toEqual(["✕", "build", "build-2.md · claude opus-5.5", ""]);
  });

  it("does not count up a turn of a stale run, which nothing drives", () => {
    render(<RunMap record={record({ health: "stale" })} now={NOW} />);
    expect(screen.getByText("build · claude opus-5.5 · stale")).toBeInTheDocument();
    const [newest] = within(screen.getByRole("list", { name: "turns" })).getAllByRole("listitem");
    expect([newest?.firstChild?.textContent, newest?.lastChild?.textContent]).toEqual(["–", "unfinished"]);
  });

  it("keeps eight arms, the spare ones curled", () => {
    const { container } = render(<RunMap record={record()} now={NOW} />);
    expect(container.querySelectorAll(".arm")).toHaveLength(8);
    expect(container.querySelectorAll(".arm.spare")).toHaveLength(3);
  });

  it("animates until unmounted, and not at all under reduced motion", () => {
    const request = vi.fn(() => 7);
    const cancel = vi.fn();
    vi.stubGlobal("requestAnimationFrame", request);
    vi.stubGlobal("cancelAnimationFrame", cancel);
    const { unmount } = render(<RunMap record={record()} now={NOW} />);
    expect(request).toHaveBeenCalled();
    unmount();
    expect(cancel).toHaveBeenCalledWith(7);

    request.mockClear();
    const reduce = mediaQuery(true);
    vi.stubGlobal("matchMedia", () => reduce);
    const { container } = render(<RunMap record={record()} now={NOW} />);
    expect(request).not.toHaveBeenCalled();
    expect(container.querySelector("[data-pulse]")).toBeNull();
    expect(stations()).toHaveLength(5);
  });

  it("stops moving when the viewer turns on reduced motion while it shows", () => {
    const cancel = vi.fn();
    vi.stubGlobal("requestAnimationFrame", vi.fn(() => 9));
    vi.stubGlobal("cancelAnimationFrame", cancel);
    const reduce = mediaQuery(false);
    vi.stubGlobal("matchMedia", () => reduce);
    const { container } = render(<RunMap record={record()} now={NOW} />);
    expect(container.querySelector("[data-pulse]")).not.toBeNull();
    act(() => reduce.set(true));
    expect(cancel).toHaveBeenCalledWith(9);
    expect(container.querySelector("[data-pulse]")).toBeNull();
  });
});

/** A stand-in for the reduced-motion MediaQueryList, matching as matches says until set changes it and tells its
 * listeners. */
function mediaQuery(matches: boolean) {
  const listeners = new Set<() => void>();
  return {
    get matches() {
      return matches;
    },
    addEventListener: (_: string, listener: () => void) => listeners.add(listener),
    removeEventListener: (_: string, listener: () => void) => listeners.delete(listener),
    set(now: boolean) {
      matches = now;
      listeners.forEach((listener) => listener());
    },
  };
}
