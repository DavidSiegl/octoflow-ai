import { type FormEvent, useEffect, useRef, useState } from "react";
import type { AcpPermissionOption, AcpToolCall, AgentEvent, Api, Project } from "../api";
import { useFollowEnd } from "../hooks";
import { AGENT_POLL_MS } from "./AgentPanes";

/** What the conversation shows for the events of a session, in the order they came: chunks of one kind that follow
 * each other are one item, and a tool call's updates change the item its tool_call made. */
type Item =
  | { kind: "session"; key: string; harness: string; resumed: boolean }
  | { kind: "prompt"; key: string; text: string; from: "orchestrator" | "human" }
  | { kind: "text"; key: string; text: string }
  | { kind: "thought"; key: string; text: string }
  | { kind: "tool"; key: string; call: AcpToolCall }
  | { kind: "permission"; key: string; request: number; call: AcpToolCall | null; options: AcpPermissionOption[] }
  | { kind: "turn_end"; key: string; stopReason: string | null }
  | { kind: "exited"; key: string; error: string | null };

/** What the message box says the agent waits for, by the journal's `waiting` why. */
const WAITING_FOR: Record<string, string> = {
  interview: "The agent is waiting for your answer.",
  blocked: "The agent is blocked and waits for your answer to its question.",
  idle: "The agent is idle without writing its file. Tell it how to go on.",
};

interface Props {
  api: Api;
  project: Project;
  run: string;
  role: string;
}

/** The current session of a run's agent over ACP, read again every AGENT_POLL_MS while shown: what the agent says
 * and thinks (thoughts collapsed), its tool calls with their status, its permission requests with a button per
 * option, and, while the run waits on the human's message to it, a box to send one. Nothing can be answered while the
 * run is not running. */
export function Conversation({ api, project, run, role }: Props) {
  const { events, answerable, problem, refresh } = useConversation(api, project, run, role);
  const [sendProblem, setSendProblem] = useState<string | null>(null);
  const list = useFollowEnd<HTMLOListElement>(events);

  const answers = new Map<number, { option: string; by: string }>();
  for (const e of events) {
    if (e.kind === "answered") {
      answers.set(e.request, { option: e.option, by: e.by });
    }
  }
  const waiting = pendingWait(events);
  const settle = (sent: Promise<void>) =>
    sent.then(
      () => {
        setSendProblem(null);
        refresh();
        return true;
      },
      (error: Error) => {
        setSendProblem(error.message);
        refresh();
        return false;
      },
    );

  return (
    <div className="agent conversation" role="tabpanel" aria-label={`${role} agent`}>
      {problem && <p className="problem">{problem}</p>}
      {!answerable && events.length > 0 && (
        <p className="note">The run is not running, so its agents cannot be answered here.</p>
      )}
      <ol ref={list} aria-label={`what the ${role} agent did`}>
        {items(events).map((item) => (
          <li key={item.key} className={`item ${item.kind}`}>
            <ItemView
              item={item}
              answer={item.kind === "permission" ? answers.get(item.request) : undefined}
              answerable={answerable}
              onAnswer={(request, option) =>
                void settle(api.answerPermission(project, run, role, request, option))
              }
            />
          </li>
        ))}
      </ol>
      {waiting && answerable && (
        <MessageBox
          role={role}
          why={waiting}
          send={(text) => settle(api.messageAgent(project, run, role, text))}
        />
      )}
      {sendProblem && <p className="problem">{sendProblem}</p>}
    </div>
  );
}

/** One item of the conversation. answer is a permission's, once answered. */
function ItemView({
  item,
  answer,
  answerable,
  onAnswer,
}: {
  item: Item;
  answer: { option: string; by: string } | undefined;
  answerable: boolean;
  onAnswer: (request: number, option: string) => void;
}) {
  switch (item.kind) {
    case "session":
      return (
        <p className="session">
          {item.resumed ? "Resumed its session" : "Started a session"} in {item.harness}
        </p>
      );
    case "prompt":
      return (
        <div className={`prompt from-${item.from}`}>
          <span className="who">{item.from === "human" ? "You" : "octoflow"}</span>
          <p>{item.text}</p>
        </div>
      );
    case "text":
      return <p className="said">{item.text}</p>;
    case "thought":
      return (
        <details className="thought">
          <summary>Thinking</summary>
          <p>{item.text}</p>
        </details>
      );
    case "tool":
      return <ToolCall call={item.call} />;
    case "permission": {
      const chosen = answer && item.options.find((o) => o.optionId === answer.option);
      return (
        <div className="permission" role="group" aria-label={`permission ${item.request}`}>
          <p>
            The agent asks to run <strong>{item.call?.title ?? "a tool"}</strong>
          </p>
          {answer ? (
            <p className="answered">
              {answer.by === "stop" ? "Cancelled when the run stopped" : `Answered: ${chosen?.name ?? answer.option}`}
            </p>
          ) : (
            <div className="options">
              {item.options.map((o) => (
                <button
                  key={o.optionId}
                  type="button"
                  disabled={!answerable}
                  onClick={() => onAnswer(item.request, o.optionId)}
                >
                  {o.name}
                </button>
              ))}
            </div>
          )}
        </div>
      );
    }
    case "turn_end":
      return <p className="turn-end">{item.stopReason === "end_turn" ? "Done" : `Stopped: ${item.stopReason}`}</p>;
    case "exited":
      return <p className="exited problem">{item.error ? `The agent failed: ${item.error}` : "The agent exited"}</p>;
  }
}

/** A tool call's title and status. */
function ToolCall({ call }: { call: AcpToolCall }) {
  const status = call.status ?? "pending";
  return (
    <p className={`tool status-${status}`}>
      <span className="title">{call.title ?? call.toolCallId}</span> <span className="status">{status.replace("_", " ")}</span>
    </p>
  );
}

/** The box that sends the human's message, while the run waits on it for why. send resolves to whether it was sent:
 * the box is cleared only then. */
function MessageBox({ role, why, send }: { role: string; why: string; send: (text: string) => Promise<boolean> }) {
  const [draft, setDraft] = useState("");
  // True while the draft is being sent: it can be neither sent again nor changed until the send settles.
  const [sending, setSending] = useState(false);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (draft.trim() && !sending) {
      setSending(true);
      void send(draft).then((sent) => {
        setSending(false);
        if (sent) {
          setDraft("");
        }
      });
    }
  };
  return (
    <form className="message" onSubmit={submit}>
      <p>{WAITING_FOR[why] ?? WAITING_FOR.interview}</p>
      <textarea
        aria-label={`Message to the ${role} agent`}
        value={draft}
        readOnly={sending}
        onChange={(event) => setDraft(event.target.value)}
      />
      <button type="submit" disabled={!draft.trim() || sending}>
        Send
      </button>
    </form>
  );
}

/** The conversation's items for events, a session's (see Item). */
function items(events: AgentEvent[]): Item[] {
  const out: Item[] = [];
  const tools = new Map<string, { kind: "tool"; key: string; call: AcpToolCall }>();
  const last = () => out[out.length - 1];
  for (const e of events) {
    const key = String(e.seq);
    switch (e.kind) {
      case "session":
        out.push({ kind: "session", key, harness: e.harness, resumed: e.resumed });
        break;
      case "prompt":
        out.push({ kind: "prompt", key, text: e.text, from: e.from });
        break;
      case "permission":
        out.push({ kind: "permission", key, request: e.request, call: e.toolCall, options: e.options ?? [] });
        break;
      case "turn_end":
        out.push({ kind: "turn_end", key, stopReason: e.stopReason });
        break;
      case "exited":
        out.push({ kind: "exited", key, error: e.error ?? null });
        break;
      case "update": {
        const u = e.update;
        if (u.sessionUpdate === "tool_call" || u.sessionUpdate === "tool_call_update") {
          const call = u as AcpToolCall;
          const known = tools.get(call.toolCallId);
          if (known) {
            known.call = { ...known.call, ...defined(call) };
          } else {
            const item = { kind: "tool" as const, key, call: { ...call } };
            tools.set(call.toolCallId, item);
            out.push(item);
          }
          break;
        }
        const chunk = chunkKind(u.sessionUpdate);
        if (!chunk || !("content" in u) || typeof u.content.text !== "string") {
          break;
        }
        const before = last();
        if (chunk === "prompt") {
          // A reopened session's history, as the agent replays it: the prompts it was sent.
          if (before?.kind === "prompt") {
            before.text += u.content.text;
          } else {
            out.push({ kind: "prompt", key, text: u.content.text, from: "orchestrator" });
          }
        } else if (before?.kind === chunk) {
          before.text += u.content.text;
        } else {
          out.push({ kind: chunk, key, text: u.content.text });
        }
        break;
      }
    }
  }
  return out;
}

/** The item a chunk of sessionUpdate adds to: the agent's text, its thought, or a prompt it replays; null for an
 * update that is no chunk. */
function chunkKind(sessionUpdate: string): "text" | "thought" | "prompt" | null {
  switch (sessionUpdate) {
    case "agent_message_chunk":
      return "text";
    case "agent_thought_chunk":
      return "thought";
    case "user_message_chunk":
      return "prompt";
    default:
      return null;
  }
}

/** call without its undefined and null fields: what a tool_call_update changes. */
function defined(call: AcpToolCall): Partial<AcpToolCall> {
  return Object.fromEntries(Object.entries(call).filter(([, v]) => v !== undefined && v !== null));
}

/** Why the run waits on the human's message to the agent, from the session's events: the last `waiting` that no
 * `prompt` or `exited` followed; null when it does not wait. */
function pendingWait(events: AgentEvent[]): string | null {
  let why: string | null = null;
  for (const e of events) {
    if (e.kind === "waiting") {
      why = e.why;
    } else if (e.kind === "prompt" || e.kind === "exited") {
      why = null;
    }
  }
  return why;
}

/** The current session of the agent's events, read now and every AGENT_POLL_MS while mounted, one read at a time,
 * each asking only for the events after the last one held; a session that started since replaces them all.
 * answerable is the last read's; problem says why the last read failed, null once one succeeds. refresh reads again
 * at once. */
function useConversation(api: Api, project: Project, run: string, role: string) {
  const [events, setEvents] = useState<AgentEvent[]>([]);
  const [answerable, setAnswerable] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [reads, setReads] = useState(0);
  // What the reads so far hold: the session's start and its last event's seq, which the next read goes on from.
  const held = useRef<{ start: number | null; seq: number }>({ start: null, seq: 0 });
  useEffect(() => {
    let current = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const read = () => {
      api.agentEvents(project, run, role, held.current.seq).then(
        (next) => {
          if (!current) {
            return;
          }
          const fresh = next.session_start !== held.current.start;
          const last = next.events[next.events.length - 1];
          held.current = { start: next.session_start, seq: last ? last.seq : held.current.seq };
          if (fresh || next.events.length > 0) {
            setEvents((before) => (fresh ? next.events : [...before, ...next.events]));
          }
          setAnswerable(next.answerable);
          setProblem(null);
          timer = setTimeout(read, AGENT_POLL_MS);
        },
        (error: Error) => {
          if (current) {
            setProblem(error.message);
            timer = setTimeout(read, AGENT_POLL_MS);
          }
        },
      );
    };
    read();
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [api, project, run, role, reads]);
  return { events, answerable, problem, refresh: () => setReads((n) => n + 1) };
}
