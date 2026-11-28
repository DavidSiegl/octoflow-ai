import { type FormEvent, useEffect, useState } from "react";
import { type AgentInput, type AgentRef, type Api, HttpError, type Project, type RunRecord } from "../api";
import { useHeldWhileScrolledUp } from "../hooks";
import { readsAgain } from "../runs";
import { Conversation } from "./Conversation";

/** How often the shown agent's terminal is read again, in milliseconds. */
export const AGENT_POLL_MS = 2000;

/** The lines of an agent's terminal shown. */
export const AGENT_LINES = 200;

/** The keys the section offers as buttons, as herdr names them, with their labels. */
const KEYS: [string, string][] = [
  ["esc", "Esc"],
  ["enter", "Enter"],
  ["up", "↑"],
  ["down", "↓"],
  ["ctrl+c", "Ctrl+C"],
];

interface Props {
  api: Api;
  project: Project;
  record: RunRecord;
}

/** The run's agents, a tab each: for a run in herdr, what the chosen one's pane shows, read again as readsAgain says
 * while it is shown, and a box and keys to type into it, as into its herdr pane, while it has not exited; for a run
 * over ACP, its conversation (Conversation). The current role's agent is chosen until another is. */
export function AgentPanes({ api, project, record }: Props) {
  const agents = record.agents ?? [];
  const [chosen, setChosen] = useState<string | null>(null);
  const shown = agents.find((a) => a.role === chosen) ?? agents.find((a) => a.role === record.role) ?? agents[0];
  if (!shown) {
    return (
      <section className="agents" aria-label="agents">
        <p className="empty">No agent has started yet.</p>
      </section>
    );
  }
  return (
    <section className="agents" aria-label="agents">
      <div role="tablist" aria-label="agents">
        {agents.map((a) => (
          <button
            key={a.role}
            type="button"
            role="tab"
            aria-selected={a.role === shown.role}
            title={`${a.name} (${a.kind})`}
            onClick={() => setChosen(a.role)}
          >
            {a.role}
          </button>
        ))}
      </div>
      {/* Keyed by the agent, so another one starts from nothing read and nothing typed. */}
      {record.backend === "acp" ? (
        <Conversation key={shown.name} api={api} project={project} record={record} role={shown.role} />
      ) : (
        <AgentPane key={shown.name} api={api} project={project} record={record} agent={shown} />
      )}
    </section>
  );
}

/** What record's run's agent shows in its pane, and the input to it while it has not exited. */
function AgentPane({ api, project, record, agent }: { api: Api; project: Project; record: RunRecord; agent: AgentRef }) {
  const { text, problem, exited, refresh } = useAgentText(api, project, record, agent.role);
  const [draft, setDraft] = useState("");
  // True while the draft is being sent: it can be neither sent again nor changed until the send settles, so that one
  // Enter sends it once and success clears what was sent.
  const [sending, setSending] = useState(false);
  const [sendProblem, setSendProblem] = useState<string | null>(null);
  // Each read replaces the window of lines whole, so while the reader is scrolled up it would slide what they read.
  const { ref: screen, shown: screenText, held, toEnd } = useHeldWhileScrolledUp<HTMLPreElement, string | null>(text);

  const type = (input: AgentInput) =>
    api.typeInto(project, record.id, agent.role, input).then(
      () => {
        setSendProblem(null);
        // As a terminal does on input: the reader is to see what it brings, also if they had scrolled up.
        toEnd();
        refresh();
        return true;
      },
      (error: Error) => {
        setSendProblem(error.message);
        return false;
      },
    );
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (draft.trim() && !sending) {
      setSending(true);
      void type({ text: draft }).then((sent) => {
        setSending(false);
        if (sent) {
          setDraft("");
        }
      });
    }
  };

  return (
    <div className="agent" role="tabpanel" aria-label={`${agent.role} agent`}>
      {problem && <p className="problem">{problem}</p>}
      <pre ref={screen} aria-label={`what the ${agent.role} agent shows`}>
        {screenText ?? (exited ? "No output to show." : "Reading…")}
      </pre>
      {/* Mounted, empty, while nothing is held, so that a screen reader announces the text when it comes. */}
      <p className="notice" role="status">
        {held && "Held while you are scrolled up: scroll to the end for newer output."}
      </p>
      <form onSubmit={submit}>
        <input
          aria-label={`Message to the ${agent.role} agent`}
          value={draft}
          readOnly={sending}
          disabled={exited}
          onChange={(event) => setDraft(event.target.value)}
          placeholder="Type to the agent, as in its pane"
        />
        <button type="submit" disabled={!draft.trim() || sending || exited}>
          Send
        </button>
        {KEYS.map(([key, label]) => (
          <button
            key={key}
            type="button"
            aria-label={`press ${key}`}
            disabled={exited}
            onClick={() => void type({ keys: [key] })}
          >
            {label}
          </button>
        ))}
      </form>
      {sendProblem && <p className="problem">{sendProblem}</p>}
    </div>
  );
}

/** The last AGENT_LINES lines of what record's run's agent of role shows, read now and again as readsAgain says while
 * mounted, one read at a time. A 404 means the agent has exited, or never was; reading on would only have the server
 * spawn herdr to say so again. text keeps the last read that succeeded, and problem says why the last one failed, null
 * once one succeeds; exited is true while the last read answered 404. refresh reads again at once. */
function useAgentText(api: Api, project: Project, record: RunRecord, role: string) {
  const [text, setText] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [exited, setExited] = useState(false);
  const [reads, setReads] = useState(0);
  useEffect(() => {
    let current = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const read = () => {
      api.agentText(project, record.id, role, AGENT_LINES).then(
        (next) => {
          if (current) {
            setText(next);
            setProblem(null);
            setExited(false);
            if (readsAgain(record, null)) {
              timer = setTimeout(read, AGENT_POLL_MS);
            }
          }
        },
        (error: Error) => {
          if (current) {
            setProblem(error.message);
            setExited(error instanceof HttpError && error.status === 404);
            if (readsAgain(record, error)) {
              timer = setTimeout(read, AGENT_POLL_MS);
            }
          }
        },
      );
    };
    read();
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [api, project, record, role, reads]);
  return { text, problem, exited, refresh: () => setReads((n) => n + 1) };
}
