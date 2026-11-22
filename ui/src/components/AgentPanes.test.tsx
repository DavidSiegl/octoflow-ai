import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Api, RunRecord } from "../api";
import { AGENT_POLL_MS, AgentPanes } from "./AgentPanes";

const PROJECT = { cwd: "/home/me/proj", machine: "" };

describe("the herdr pane of an agent", () => {
  it("follows its end while the reader is there, and stays where they scrolled up to", async () => {
    vi.useFakeTimers();
    let lines = 0;
    // Each read shows three lines more than the last.
    const agentText = vi.fn(async () => {
      lines += 3;
      return Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    });
    const api = { agentText } as unknown as Api;
    const record = {
      id: "20261122-120000-c0ffee",
      role: "spec",
      backend: "herdr",
      agents: [{ role: "spec", name: "spec-1", kind: "claude" }],
    } as unknown as RunRecord;
    render(<AgentPanes api={api} project={PROJECT} record={record} />);
    const pane = screen.getByLabelText("what the spec agent shows");
    // jsdom lays nothing out: each line is 20px high, in a pane 100px high. Nothing fires the scroll events a browser
    // would, so each read lands before its scroll's event, as one can in a browser.
    Object.defineProperty(pane, "scrollHeight", { get: () => 20 * (pane.textContent ?? "").split("\n").length });
    Object.defineProperty(pane, "clientHeight", { value: 100 });
    const readAgain = () =>
      act(async () => {
        await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
      });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    await readAgain();
    expect(pane.scrollTop).toBe(pane.scrollHeight);

    pane.scrollTop = 0;
    await readAgain();
    expect(pane.scrollTop).toBe(0);

    pane.scrollTop = pane.scrollHeight - pane.clientHeight;
    await readAgain();
    expect(pane.scrollTop).toBe(pane.scrollHeight);
  });
});
