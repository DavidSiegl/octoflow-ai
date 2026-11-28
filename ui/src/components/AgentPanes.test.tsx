import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { type Api, HttpError, type RunRecord } from "../api";
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

describe("the herdr pane of an agent whose output outgrows the lines read", () => {
  it("keeps its text while the reader is scrolled up, and shows the newest read back at the end", async () => {
    vi.useFakeTimers();
    let lines = 0;
    // Each read shows the last five lines of an output three lines longer than the last read's, as a full window does.
    const agentText = vi.fn(async () => {
      lines += 3;
      return Array.from({ length: lines }, (_, i) => `line ${i}`)
        .slice(-5)
        .join("\n");
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
    // Each line 20px high, in a pane 60px high: five lines overflow it.
    Object.defineProperty(pane, "scrollHeight", { get: () => 20 * (pane.textContent ?? "").split("\n").length });
    Object.defineProperty(pane, "clientHeight", { value: 60 });
    const readAgain = () =>
      act(async () => {
        await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
      });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    await readAgain();
    expect(pane.textContent).toBe("line 1\nline 2\nline 3\nline 4\nline 5");

    // Scrolled up with no scroll event yet, as a read can land before it: the read that brings new lines is held.
    pane.scrollTop = 0;
    await readAgain();
    expect(pane.textContent).toBe("line 1\nline 2\nline 3\nline 4\nline 5");
    expect(screen.getByRole("status")).toHaveTextContent("newer output");
    await act(async () => {
      fireEvent.scroll(pane);
    });
    await readAgain();
    expect(pane.textContent).toBe("line 1\nline 2\nline 3\nline 4\nline 5");

    pane.scrollTop = pane.scrollHeight - pane.clientHeight;
    await act(async () => {
      fireEvent.scroll(pane);
    });
    expect(pane.textContent).toBe("line 7\nline 8\nline 9\nline 10\nline 11");
    expect(pane.scrollTop).toBe(pane.scrollHeight);
    expect(screen.getByRole("status")).toHaveTextContent("");
  });

  it("goes to its end and follows it again once input the reader sent while it held arrives", async () => {
    vi.useFakeTimers();
    let lines = 0;
    const agentText = vi.fn(async () => {
      lines += 3;
      return Array.from({ length: lines }, (_, i) => `line ${i}`)
        .slice(-5)
        .join("\n");
    });
    const typeInto = vi.fn(async () => undefined);
    const api = { agentText, typeInto } as unknown as Api;
    const record = {
      id: "20261122-120000-c0ffee",
      role: "spec",
      backend: "herdr",
      agents: [{ role: "spec", name: "spec-1", kind: "claude" }],
    } as unknown as RunRecord;
    render(<AgentPanes api={api} project={PROJECT} record={record} />);
    const pane = screen.getByLabelText("what the spec agent shows");
    Object.defineProperty(pane, "scrollHeight", { get: () => 20 * (pane.textContent ?? "").split("\n").length });
    Object.defineProperty(pane, "clientHeight", { value: 60 });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    });
    pane.scrollTop = 0;
    await act(async () => {
      fireEvent.scroll(pane);
      await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    });
    expect(pane.textContent).toBe("line 1\nline 2\nline 3\nline 4\nline 5");

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "press enter" }));
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(typeInto).toHaveBeenCalled();
    expect(pane.textContent).toBe("line 7\nline 8\nline 9\nline 10\nline 11");
    expect(pane.scrollTop).toBe(pane.scrollHeight);
    expect(screen.getByRole("status")).toHaveTextContent("");
  });
});

describe("the herdr pane of an agent that has exited", () => {
  it("says there is no output, offers nothing to type or press, and reads no more", async () => {
    vi.useFakeTimers();
    const agentText = vi.fn(async () => {
      throw new HttpError(404, "the build agent has exited");
    });
    const record = {
      id: "20261122-120000-c0ffee",
      role: "build",
      backend: "herdr",
      health: "running",
      agents: [{ role: "build", name: "build-1", kind: "claude" }],
    } as unknown as RunRecord;
    render(<AgentPanes api={{ agentText } as unknown as Api} project={PROJECT} record={record} />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3 * AGENT_POLL_MS);
    });

    expect(agentText).toHaveBeenCalledTimes(1);
    expect(screen.getByText("the build agent has exited")).toBeInTheDocument();
    expect(screen.getByLabelText("what the build agent shows")).toHaveTextContent("No output to show.");
    expect(screen.queryByText("Reading…")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Message to the build agent")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    for (const key of ["esc", "enter", "up", "down", "ctrl+c"]) {
      expect(screen.getByRole("button", { name: `press ${key}` })).toBeDisabled();
    }
  });

  it("reads again once the run's record changes, as when the engine starts the agent again", async () => {
    vi.useFakeTimers();
    const agentText = vi.fn().mockRejectedValueOnce(new HttpError(404, "the build agent has exited")).mockResolvedValue("again");
    const api = { agentText } as unknown as Api;
    const record = {
      id: "20261122-120000-c0ffee",
      role: "build",
      backend: "herdr",
      health: "running",
      agents: [{ role: "build", name: "build-1", kind: "claude" }],
    } as unknown as RunRecord;
    const { rerender } = render(<AgentPanes api={api} project={PROJECT} record={record} />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByRole("button", { name: "press esc" })).toBeDisabled();

    rerender(<AgentPanes api={api} project={PROJECT} record={{ ...record, pane: "w1:p2" }} />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(agentText).toHaveBeenCalledTimes(2);
    expect(screen.getByLabelText("what the build agent shows")).toHaveTextContent("again");
    expect(screen.getByRole("button", { name: "press esc" })).toBeEnabled();
  });
});

describe("the herdr pane of an agent of a run nothing drives", () => {
  it("is read until a read answers", async () => {
    vi.useFakeTimers();
    const agentText = vi.fn().mockRejectedValueOnce(new HttpError(502, "herdr did not answer")).mockResolvedValue("done");
    const record = {
      id: "20261122-120000-c0ffee",
      role: "build",
      backend: "herdr",
      health: null,
      agents: [{ role: "build", name: "build-1", kind: "claude" }],
    } as unknown as RunRecord;
    render(<AgentPanes api={{ agentText } as unknown as Api} project={PROJECT} record={record} />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3 * AGENT_POLL_MS);
    });

    expect(agentText).toHaveBeenCalledTimes(2);
    expect(screen.getByLabelText("what the build agent shows")).toHaveTextContent("done");
  });
});
