import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type AgentEvent, type Api, HttpError, type RunRecord, type Conversation as Session } from "../api";
import { AGENT_POLL_MS } from "./AgentPanes";
import { Conversation } from "./Conversation";

const PROJECT = { cwd: "/home/me/proj" };
const RUN = "20261024-140000-c0ffee";
/** A record of RUN, driven. */
const RECORD = { id: RUN, health: "running" } as RunRecord;

/** An event of kind with its fields, numbered seq. */
function event(seq: number, fields: Record<string, unknown>): AgentEvent {
  return { seq, at: "2026-10-24T14:00:00Z", ...fields } as AgentEvent;
}

/** A session of every kind of event the conversation shows, up to a wait on the human for why. */
function session(why: string | null): AgentEvent[] {
  const events = [
    event(1, { kind: "session", session: "s-1", harness: "agy", resumed: false }),
    event(2, { kind: "prompt", text: "Write the spec", from: "orchestrator" }),
    event(3, { kind: "update", update: { sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "pondering" } } }),
    event(4, { kind: "update", update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Which " } } }),
    event(5, { kind: "update", update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "limit?" } } }),
    event(6, { kind: "update", update: { sessionUpdate: "tool_call", toolCallId: "t1", title: "Read src/lib.rs", status: "pending" } }),
    event(7, { kind: "update", update: { sessionUpdate: "tool_call_update", toolCallId: "t1", status: "completed" } }),
    event(8, { kind: "update", update: { sessionUpdate: "plan", entries: [] } }),
    event(9, {
      kind: "permission",
      request: 1,
      toolCall: { toolCallId: "t2", title: "rm -rf target" },
      options: [
        { optionId: "allow", name: "Allow", kind: "allow_once" },
        { optionId: "deny", name: "Deny", kind: "reject_once" },
      ],
    }),
    event(10, { kind: "turn_end", stopReason: "end_turn" }),
  ];
  return why ? [...events, event(11, { kind: "waiting", why })] : events;
}

/** An Api whose agentEvents answers sessions in turn, the last again and again, each from the seq asked for; and whose
 * answerPermission and messageAgent resolve, or reject with refused. */
function fakeApi(sessions: Session[], refused?: string) {
  let read = 0;
  const agentEvents = vi.fn(async (_p: unknown, _r: string, _role: string, after: number) => {
    const s = sessions[Math.min(read++, sessions.length - 1)] as Session;
    return { ...s, events: s.events.filter((e) => e.seq > after) };
  });
  const settle = vi.fn(async () => {
    if (refused) {
      throw new Error(refused);
    }
  });
  const api = { agentEvents, answerPermission: settle, messageAgent: settle } as unknown as Api;
  return { api, agentEvents, settle };
}

/** Render the spec agent's conversation of RECORD's run; returns its list. */
function show(api: Api) {
  render(<Conversation api={api} project={PROJECT} record={RECORD} role="spec" />);
  return screen.getByRole("list", { name: "what the spec agent did" });
}

afterEach(() => {
  vi.useRealTimers();
  // jsdom has no scrollIntoView; a test that watches for one puts it there.
  delete (Element.prototype as Partial<Element>).scrollIntoView;
});

describe("the conversation of an agent over ACP", () => {
  it("shows text, thoughts collapsed, tool calls with their latest status and permissions with their options", async () => {
    const { api } = fakeApi([{ session_start: 1, events: session(null), answerable: true }]);
    const list = show(api);

    expect(await within(list).findByText("Which limit?")).toBeInTheDocument();
    expect(within(list).getByText("Started a session in agy")).toBeInTheDocument();
    expect(within(list).getByText("Write the spec")).toBeInTheDocument();
    const thought = within(list).getByText("pondering").closest("details");
    expect(thought).not.toHaveAttribute("open");
    expect(within(list).getByText("Read src/lib.rs").parentElement).toHaveTextContent("Read src/lib.rs completed");
    const permission = within(list).getByRole("group", { name: "permission 1" });
    expect(permission).toHaveTextContent("The agent asks to run rm -rf target");
    expect(within(permission).getAllByRole("button").map((b) => b.textContent)).toEqual(["Allow", "Deny"]);
    expect(within(list).getByText("Done")).toBeInTheDocument();
    expect(screen.queryByLabelText("Message to the spec agent")).not.toBeInTheDocument();
  });

  it("answers a permission by its option, and shows the answer the journal then holds", async () => {
    const answered = [...session(null), event(11, { kind: "answered", request: 1, option: "deny", by: "human" })];
    const { api, settle } = fakeApi([
      { session_start: 1, events: session(null), answerable: true },
      { session_start: 1, events: answered, answerable: true },
    ]);
    const list = show(api);
    const permission = await within(list).findByRole("group", { name: "permission 1" });

    fireEvent.click(within(permission).getByRole("button", { name: "Deny" }));

    expect(settle).toHaveBeenCalledWith(PROJECT, RUN, "spec", 1, "deny");
    expect(await within(permission).findByText("Answered: Deny")).toBeInTheDocument();
    expect(within(permission).queryByRole("button")).not.toBeInTheDocument();
  });

  it("sends one answer to a request clicked twice, or on two of its options", async () => {
    const { api, settle } = fakeApi([{ session_start: 1, events: session(null), answerable: true }]);
    const list = show(api);
    const permission = await within(list).findByRole("group", { name: "permission 1" });

    fireEvent.click(within(permission).getByRole("button", { name: "Allow" }));
    fireEvent.click(within(permission).getByRole("button", { name: "Allow" }));
    fireEvent.click(within(permission).getByRole("button", { name: "Deny" }));

    expect(settle).toHaveBeenCalledTimes(1);
    // The answer took, but the journal has yet to say so: the options stay disabled until its answered event.
    await act(async () => settle.mock.results[0]?.value);
    expect(within(permission).getByRole("button", { name: "Allow" })).toBeDisabled();
    expect(within(permission).getByRole("button", { name: "Deny" })).toBeDisabled();
  });

  it("offers a request's options again once its answer is refused, and says why", async () => {
    const { api } = fakeApi([{ session_start: 1, events: session(null), answerable: true }], "the run is not running");
    const list = show(api);
    const permission = await within(list).findByRole("group", { name: "permission 1" });

    fireEvent.click(within(permission).getByRole("button", { name: "Deny" }));

    expect(await screen.findByText("the run is not running")).toBeInTheDocument();
    expect(within(permission).getByRole("button", { name: "Deny" })).toBeEnabled();
  });

  it.each([
    ["interview", "The agent is waiting for your answer."],
    ["blocked", "The agent is blocked and waits for your answer to its question."],
    ["idle", "The agent is idle without writing its file. Tell it how to go on."],
  ])("offers a message box while the run waits on the human for the %s, and sends the message", async (why, says) => {
    const { api, settle } = fakeApi([{ session_start: 1, events: session(why), answerable: true }]);
    show(api);
    const box = await screen.findByLabelText("Message to the spec agent");
    expect(screen.getByText(says)).toBeInTheDocument();

    fireEvent.change(box, { target: { value: "per client" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    expect(settle).toHaveBeenCalledWith(PROJECT, RUN, "spec", "per client");
    await waitFor(() => expect(box).toHaveValue(""));
  });

  it("keeps a message that was refused, and says why", async () => {
    const { api } = fakeApi([{ session_start: 1, events: session("interview"), answerable: true }], "not waiting");
    show(api);
    const box = await screen.findByLabelText("Message to the spec agent");

    fireEvent.change(box, { target: { value: "per client" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    expect(await screen.findByText("not waiting")).toBeInTheDocument();
    expect(box).toHaveValue("per client");
  });

  it("offers no box once a prompt follows the wait, and nothing to answer while the run is not running", async () => {
    const prompted = [...session("interview"), event(12, { kind: "prompt", text: "per client", from: "human" })];
    const { api } = fakeApi([{ session_start: 1, events: prompted, answerable: false }]);
    const list = show(api);

    expect(await within(list).findByText("per client")).toBeInTheDocument();
    expect(screen.queryByLabelText("Message to the spec agent")).not.toBeInTheDocument();
    expect(within(list).getByRole("button", { name: "Allow" })).toBeDisabled();
    expect(screen.getByText("The run is not running, so its agents cannot be answered here.")).toBeInTheDocument();
  });

  it("keeps the message being typed while the box is hidden and shown again", async () => {
    vi.useFakeTimers();
    const waiting = session("interview");
    const { api } = fakeApi([
      { session_start: 1, events: waiting, answerable: true },
      { session_start: 1, events: waiting, answerable: false },
      { session_start: 1, events: waiting, answerable: true },
    ]);
    show(api);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    fireEvent.change(screen.getByLabelText("Message to the spec agent"), { target: { value: "per cli" } });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    });
    expect(screen.queryByLabelText("Message to the spec agent")).not.toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    });
    expect(screen.getByLabelText("Message to the spec agent")).toHaveValue("per cli");
  });

  it("sends a message once while its box is hidden and shown again during the send", async () => {
    vi.useFakeTimers();
    let release = () => {};
    const waiting = session("interview");
    const { api, settle } = fakeApi([
      { session_start: 1, events: waiting, answerable: true },
      { session_start: 1, events: waiting, answerable: false },
      { session_start: 1, events: waiting, answerable: true },
    ]);
    settle.mockImplementation(() => new Promise<void>((resolve) => (release = resolve)));
    show(api);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    fireEvent.change(screen.getByLabelText("Message to the spec agent"), { target: { value: "per client" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    // One act per poll: React renders an act's updates only when it ends, so one act for both would never hide it.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    });
    expect(screen.queryByLabelText("Message to the spec agent")).not.toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    });
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await act(async () => release());
    expect(settle).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText("Message to the spec agent")).toHaveValue("");
  });

  it("starts a new session's message empty", async () => {
    vi.useFakeTimers();
    const resumed = [...session("interview"), event(12, { kind: "session", session: "s-1", harness: "agy", resumed: true }), event(13, { kind: "waiting", why: "interview" })];
    const { api } = fakeApi([
      { session_start: 1, events: session("interview"), answerable: true },
      { session_start: 12, events: resumed, answerable: true },
    ]);
    show(api);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    fireEvent.change(screen.getByLabelText("Message to the spec agent"), { target: { value: "per cli" } });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    });
    expect(screen.getByLabelText("Message to the spec agent")).toHaveValue("");
  });

  it("reads on from the last event held, and starts over when a new session starts", async () => {
    vi.useFakeTimers();
    const resumed = [
      event(12, { kind: "session", session: "s-1", harness: "agy", resumed: true }),
      event(13, { kind: "exited", error: "not logged in" }),
    ];
    const { api, agentEvents } = fakeApi([
      { session_start: 1, events: session(null), answerable: true },
      { session_start: 12, events: [...session(null), ...resumed], answerable: true },
    ]);
    const list = show(api);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(within(list).getByText("Which limit?")).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });

    expect(agentEvents.mock.calls.map((c) => c[3])).toEqual([0, 10]);
    expect(within(list).queryByText("Which limit?")).not.toBeInTheDocument();
    expect(within(list).getByText("Resumed its session in agy")).toBeInTheDocument();
    expect(within(list).getByText("The agent failed: not logged in")).toBeInTheDocument();
  });

  it("stops reading at a 404, shows what it last had, and reads again once the run's record changes", async () => {
    vi.useFakeTimers();
    const agentEvents = vi
      .fn()
      .mockResolvedValueOnce({ session_start: 1, events: session(null), answerable: true })
      .mockRejectedValueOnce(new HttpError(404, "no journal of the spec agent"))
      .mockResolvedValue({ session_start: 1, events: [], answerable: true });
    const api = { agentEvents } as unknown as Api;
    const { rerender } = render(<Conversation api={api} project={PROJECT} record={RECORD} role="spec" />);
    const list = screen.getByRole("list", { name: "what the spec agent did" });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3 * AGENT_POLL_MS);
    });
    expect(agentEvents).toHaveBeenCalledTimes(2);
    expect(within(list).getByText("Which limit?")).toBeInTheDocument();
    expect(screen.getByText("no journal of the spec agent")).toBeInTheDocument();

    // The engine started the agent again: the run's record changed.
    rerender(<Conversation api={api} project={PROJECT} record={{ ...RECORD }} role="spec" />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(agentEvents.mock.calls.map((c) => c[3])).toEqual([0, 10, 10]);
    expect(screen.queryByText("no journal of the spec agent")).not.toBeInTheDocument();
  });

  it("reads a run nothing drives until a read answers, and again when its record changes", async () => {
    vi.useFakeTimers();
    const finished = { ...RECORD, health: null } as RunRecord;
    const agentEvents = vi
      .fn()
      .mockRejectedValueOnce(new HttpError(502, "the project cannot be reached"))
      .mockResolvedValue({ session_start: 1, events: session(null), answerable: false });
    const api = { agentEvents } as unknown as Api;
    const { rerender } = render(<Conversation api={api} project={PROJECT} record={finished} role="spec" />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5 * AGENT_POLL_MS);
    });
    expect(agentEvents).toHaveBeenCalledTimes(2);
    expect(screen.getByText("Which limit?")).toBeInTheDocument();

    rerender(<Conversation api={api} project={PROJECT} record={{ ...finished, health: "running" }} role="spec" />);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(AGENT_POLL_MS);
    });
    expect(agentEvents.mock.calls.map((c) => c[3])).toEqual([0, 0, 10, 10]);
  });

  it("follows its end while the reader is there, stays where they scrolled up to, and leaves the page where it is", async () => {
    vi.useFakeTimers();
    const intoView = vi.fn();
    Element.prototype.scrollIntoView = intoView;
    const all = session(null);
    const restarted = [event(12, { kind: "session", session: "s-1", harness: "agy", resumed: true })];
    const { api } = fakeApi([
      ...[4, 7, 9, 10].map((n) => ({ session_start: 1, events: all.slice(0, n), answerable: true })),
      { session_start: 12, events: restarted, answerable: true },
    ]);
    const list = show(api);
    // jsdom lays nothing out: each item is 100px high, in a list 200px high. Nothing fires the scroll events a browser
    // would, a frame after the reader scrolls, so each read lands before its scroll's event, as one can in a browser.
    Object.defineProperty(list, "scrollHeight", { get: () => 100 * list.children.length });
    Object.defineProperty(list, "clientHeight", { value: 200 });
    const readAgain = () =>
      act(async () => {
        await vi.advanceTimersByTimeAsync(2000);
      });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    await readAgain();
    expect(list.scrollTop).toBe(list.scrollHeight);

    list.scrollTop = 0;
    await readAgain();
    expect(list.scrollTop).toBe(0);

    list.scrollTop = list.scrollHeight - list.clientHeight;
    await readAgain();
    expect(list.scrollTop).toBe(list.scrollHeight);

    list.scrollTop = 0;
    await readAgain();
    expect(within(list).getByText("Resumed its session in agy")).toBeInTheDocument();
    expect(list.scrollTop).toBe(list.scrollHeight);
    expect(intoView).not.toHaveBeenCalled();
  });
});
