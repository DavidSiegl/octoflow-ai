import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentEvent, Api, Conversation as Session } from "../api";
import { Conversation } from "./Conversation";

const PROJECT = { cwd: "/home/me/proj" };
const RUN = "20261024-140000-c0ffee";

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

function show(api: Api) {
  render(<Conversation api={api} project={PROJECT} run={RUN} role="spec" />);
  return screen.getByRole("list", { name: "what the spec agent did" });
}

afterEach(() => {
  vi.useRealTimers();
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
});
