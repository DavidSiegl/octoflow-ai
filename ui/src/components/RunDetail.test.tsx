import { act, render, screen } from "@testing-library/react";
import type { ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";
import type { Api, RunRecord } from "../api";
import { RunDetail } from "./RunDetail";

// How many times each costly part of the run detail rendered: the handoff file's Markdown, the agent's conversation
// and the octopus.
const renders = vi.hoisted(() => ({ markdown: 0, conversation: 0, map: 0 }));

vi.mock("react-markdown", async (importOriginal) => {
  const { default: Markdown } = await importOriginal<typeof import("react-markdown")>();
  return {
    default: (props: ComponentProps<typeof Markdown>) => {
      renders.markdown += 1;
      return <Markdown {...props} />;
    },
  };
});
vi.mock("./Conversation", async (importOriginal) => {
  const { Conversation } = await importOriginal<typeof import("./Conversation")>();
  return {
    Conversation: (props: ComponentProps<typeof Conversation>) => {
      renders.conversation += 1;
      return <Conversation {...props} />;
    },
  };
});
vi.mock("./RunMap", async (importOriginal) => {
  const { RunMap } = await importOriginal<typeof import("./RunMap")>();
  return {
    RunMap: (props: ComponentProps<typeof RunMap>) => {
      renders.map += 1;
      return <RunMap {...props} />;
    },
  };
});

const NOW = Date.parse("2026-11-23T10:00:00Z");

/** An ACP run in its build step, whose build turn started 30 s before NOW and whose heartbeat was 1 s old at NOW. */
function record(): RunRecord {
  return {
    id: "20261123-100000-c0ffee",
    key: "c0ffee",
    task: "add a rate limiter",
    workflow: "default",
    steps: [
      { id: "spec", role: "spec", role_label: "Spec", loop_to: null, quality_gated: false },
      { id: "build", role: "build", role_label: "Builder", loop_to: null, quality_gated: false },
    ],
    pull_request: false,
    phase: "build",
    round: 1,
    quality_round: null,
    role: "build",
    agents: [{ role: "build", name: "build-c0ffee", kind: "claude" }],
    backend: "acp",
    health: "running",
    health_reason: null,
    heartbeat_age: 1,
    waiting: null,
    verdict: null,
    verdicts: [],
    error: null,
    closed: null,
    turns: [
      {
        step: "build",
        role: "build",
        round: 1,
        quality_round: 0,
        file: "build-1.md",
        agent: "claude",
        model: null,
        unoffered_model: null,
        session: "s-1",
        tokens: null,
        human_paced: false,
        started_at: new Date(NOW - 30_000).toISOString(),
        ended_at: null,
        seconds: null,
      },
    ],
    resumable: false,
  } as unknown as RunRecord;
}

describe("the run detail", () => {
  it("ages its heartbeat and running turn every second without rendering the file, the agent or the octopus again", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
    const api = {
      fileText: async () => "# The spec\n",
      agentEvents: async () => ({ session_start: null, events: [], answerable: false }),
    } as unknown as Api;
    const entry = { record: record(), receivedAt: NOW, files: [{ name: "spec.md", mtime: 1, size: 10 }] };
    const { container } = render(<RunDetail api={api} project={{ cwd: "/home/me/proj" }} entry={entry} onClose={() => {}} />);
    const nowLine = () => container.querySelector(".now")?.textContent;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByRole("heading", { name: "The spec" })).toBeInTheDocument();
    expect(screen.getByText(/beat 1s ago/)).toBeInTheDocument();
    expect(nowLine()).toBe("build · claude · running 30s");
    const before = { ...renders };

    // One tick, shorter than the conversation's AGENT_POLL_MS, so only the clock has run.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(screen.getByText(/beat 2s ago/)).toBeInTheDocument();
    expect(nowLine()).toBe("build · claude · running 31s");
    expect(renders).toEqual(before);
  });

  it("says a run with nothing to publish had nothing to publish, where its pull request would be", () => {
    const api = {
      fileText: async () => "",
      agentEvents: async () => ({ session_start: null, events: [], answerable: false }),
    } as unknown as Api;
    const nothing = { ...record(), pull_request: true, phase: "done", verdict: "APPROVE", nothing_to_publish: true };
    const shown = (r: RunRecord) =>
      render(<RunDetail api={api} project={{ cwd: "/home/me/proj" }} entry={{ record: r, receivedAt: NOW, files: [] }} onClose={() => {}} />);

    const { unmount } = shown(nothing);
    expect(screen.getByText(/^No pull request: nothing tracked changed/)).toBeInTheDocument();
    unmount();

    shown({ ...nothing, nothing_to_publish: false });
    expect(screen.queryByText(/^No pull request:/)).not.toBeInTheDocument();
  });
});
