import { type FormEvent, useEffect, useState } from "react";
import { type AgentInput, type AgentRef, type Api, HttpError, type Project, type RunRecord } from "../api";
import { useHeldWhileScrolledUp } from "../hooks";
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

/** The run's agents, a tab each: for a run in herdr, what the chosen one's pane shows, read again every AGENT_POLL_MS
 * while it is shown, and a box and keys to type into it, as into its herdr pane; for a run over ACP, its conversation
 * (Conversation). The current role's agent is chosen until another is. */
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
        <Conversation key={shown.name} api={api} project={project} run={record.id} role={shown.role} />
      ) : (
        <AgentPane key={shown.name} api={api} project={project} run={record.id} agent={shown} />
      )}
    </section>
  );
}

/** What one agent's pane shows, and the input to it. */
function AgentPane({ api, project, run, agent }: { api: Api; project: Project; run: string; agent: AgentRef }) {
  const { text, problem, refresh } = useAgentText(api, project, run, agent.role);
  const [draft, setDraft] = useState("");
  // True while the draft is being sent: it can be neither sent again nor changed until the send settles, so that one
  // Enter sends it once and success clears what was sent.
  const [sending, setSending] = useState(false);
  const [sendProblem, setSendProblem] = useState<string | null>(null);
  // Each read replaces the window of lines whole, so while the reader is scrolled up it would slide what they read.
  const { ref: screen, shown, held } = useHeldWhileScrolledUp<HTMLPreElement, string | null>(text);

  const type = (input: AgentInput) =>
    api.typeInto(project, run, agent.role, input).then(
      () => {
        setSendProblem(null);
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
        {shown ?? "Reading…"}
      </pre>
      {held && (
        <p className="notice" role="status">
          Held while you are scrolled up: scroll to the end for newer output.
        </p>
      )}
      <form onSubmit={submit}>
        <input
          aria-label={`Message to the ${agent.role} agent`}
          value={draft}
          readOnly={sending}
          onChange={(event) => setDraft(event.target.value)}
          placeholder="Type to the agent, as in its pane"
        />
        <button type="submit" disabled={!draft.trim() || sending}>
          Send
        </button>
        {KEYS.map(([key, label]) => (
          <button key={key} type="button" aria-label={`press ${key}`} onClick={() => void type({ keys: [key] })}>
            {label}
          </button>
        ))}
      </form>
      {sendProblem && <p className="problem">{sendProblem}</p>}
    </div>
  );
}

/** The last AGENT_LINES lines of what the agent shows, read now and every AGENT_POLL_MS while mounted, one read at a
 * time, until a read answers 404: the agent has exited, or never was, and the server would only spawn herdr to say so
 * again. text keeps the last read that succeeded, and problem says why the last one failed, null once one succeeds.
 * refresh reads again at once, and resumes reading after a 404. */
function useAgentText(api: Api, project: Project, run: string, role: string) {
  const [text, setText] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [reads, setReads] = useState(0);
  useEffect(() => {
    let current = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const read = () => {
      api.agentText(project, run, role, AGENT_LINES).then(
        (next) => {
          if (current) {
            setText(next);
            setProblem(null);
            timer = setTimeout(read, AGENT_POLL_MS);
          }
        },
        (error: Error) => {
          if (current) {
            setProblem(error.message);
            if (!(error instanceof HttpError && error.status === 404)) {
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
  }, [api, project, run, role, reads]);
  return { text, problem, refresh: () => setReads((n) => n + 1) };
}
