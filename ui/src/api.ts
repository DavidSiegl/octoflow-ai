// The protocol of `orchestrator serve`: its run records, file lists, events and agents, and the setup checks. The engine's RunServer
// (engine/src/serve.rs) and RunWatch document the endpoints and the events, and run_record every key of a record.

/** Where the sidecar's server listens, as the Tauri shell got it from `serve`'s first line. */
export interface Connection {
  url: string;
  token: string;
  /** The project to show first: the checkout the sidecar's orchestrator.py is in; null for a release's bundled
   * orchestrator, which is in none. */
  project: string | null;
}

/** A project as the server takes it: a directory, on the saved herdr machine or, with machine empty, here. */
export interface Project {
  cwd: string;
  machine: string;
}

export interface Step {
  id: string;
  role: string;
}

/** A role's agent that the run has started, by its herdr name and harness. */
export interface AgentRef {
  role: string;
  name: string;
  kind: string;
}

/** What the window types into an agent: text submitted as a prompt, or keys pressed in order, as herdr names them
 * (`esc`, `enter`, `up`, `ctrl+c`). */
export type AgentInput = { text: string } | { keys: string[] };

export interface Turn {
  step: string;
  role: string;
  round: number;
  started_at: string;
  ended_at: string | null;
}

/** One run as `list --json` prints it (orchestrator.py's run_record); null means the key does not apply. */
export interface RunRecord {
  id: string;
  key: string;
  task: string;
  workflow: string;
  steps: Step[] | null;
  phase: string;
  round: number;
  quality_round: number | null;
  role: string | null;
  pane: string | null;
  /** The agents the run has started, in the order it started them; absent from a server before 0.51.0. */
  agents?: AgentRef[];
  health: "running" | "stale" | null;
  health_reason: string | null;
  /** Seconds since state.json was written, when the server read it. */
  heartbeat_age: number;
  waiting: { reason: "blocked" | "interview" | "stale"; detail: string } | null;
  verdict: string | null;
  error: string | null;
  closed: string | null;
  branch: string | null;
  worktree: string | null;
  pr: { url: string; state: string | null; error: string | null } | null;
  turns: Turn[];
  resume_command: string | null;
}

/** A file in a run directory; mtime is in seconds since the epoch. */
export interface RunFile {
  name: string;
  mtime: number;
}

/** An event of the server's /events stream, its SSE event name as kind. */
export type RunEvent =
  | { kind: "record"; run: string; record: RunRecord | null }
  | { kind: "files"; run: string; files: RunFile[] }
  | { kind: "problem"; error: string | null };

export const EVENT_KINDS = ["record", "files", "problem"] as const;

/** One thing a run needs, as `orchestrator doctor` checks it: `required` for any run, `needed-for-pr` for a run
 * that opens a pull request, `optional` for some runs only. When not ok, detail says why and fix what to run or do;
 * fix is empty when ok. */
export interface SetupCheck {
  name: string;
  level: "required" | "needed-for-pr" | "optional";
  ok: boolean;
  detail: string;
  fix: string;
}

/** A project's setup checks, in the order the server made them; passed is true when every `required` one is ok. */
export interface SetupReport {
  checks: SetupCheck[];
  passed: boolean;
}

/** A request the server did not answer as asked, with the HTTP status it answered and its error message. */
export class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = "HttpError";
  }
}

/** The server's endpoints for one connection. Their requests reject with an HttpError for an answer they cannot use. */
export class Api {
  constructor(private readonly connection: Connection) {}

  /** The URL of the project's event stream, the token in it, since an EventSource cannot set a header. */
  eventsUrl(project: Project): string {
    return this.url("/events", project, { token: this.connection.token });
  }

  /** The text of a run's file; rejects with the server's error message for a missing file or a failed read. */
  async fileText(project: Project, run: string, name: string): Promise<string> {
    return this.text(this.url("/file", project, { run, name }));
  }

  /** The last lines of what the run's agent of role shows; rejects with the server's error message, with status 404
   * for no such run or agent or one that has exited, which reading again will not change. */
  async agentText(project: Project, run: string, role: string, lines: number): Promise<string> {
    return this.text(this.url("/agent", project, { run, role, lines: String(lines) }));
  }

  /** Type input into the run's agent of role, without waiting for the agent to take it; rejects with the server's
   * error message. */
  async typeInto(project: Project, run: string, role: string, input: AgentInput): Promise<void> {
    await this.json(this.url("/agent/input", project, { run, role }), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
    });
  }

  /** What the project needs for a run, checked where its agents run. The checks take a few seconds, longer on a
   * machine; a check that fails is part of the report, not a rejection. */
  async setupChecks(project: Project): Promise<SetupReport> {
    return this.json(this.url("/setup", project, {})) as Promise<SetupReport>;
  }

  /** Whether the setup dialog has been closed with every required check passed, on this computer, for any project;
   * rejects when the server cannot tell. */
  async setupChecked(): Promise<boolean> {
    const body = (await this.json(`${this.connection.url}/setup/checked`)) as { checked: boolean };
    return body.checked;
  }

  /** Record that the setup dialog has been closed with every required check passed, so that it no longer opens by
   * itself; rejects with why it could not be recorded. */
  async markSetupChecked(): Promise<void> {
    await this.json(`${this.connection.url}/setup/checked`, { method: "POST" });
  }

  /** The text field of a GET of url's JSON; rejects with the server's error message, or the status. */
  private async text(url: string): Promise<string> {
    const body = (await this.json(url)) as { text?: string };
    if (body.text === undefined) {
      throw new HttpError(200, "the answer has no text");
    }
    return body.text;
  }

  /** The JSON body of a request of url, with the token and init; rejects with an HttpError carrying the server's
   * error message, or the status, for any answer but a 2xx, and for a 2xx whose body is not JSON. */
  private async json(url: string, init: RequestInit = {}): Promise<unknown> {
    const headers = { ...init.headers, Authorization: `Bearer ${this.connection.token}` };
    const response = await fetch(url, { ...init, headers });
    // undefined: the body is not JSON, which an error answer may be, but no 2xx one.
    const body = (await response.json().catch(() => undefined)) as { error?: string } | undefined;
    if (!response.ok) {
      throw new HttpError(response.status, body?.error ?? `HTTP ${response.status}`);
    }
    if (body === undefined) {
      throw new HttpError(response.status, "the answer is not JSON");
    }
    return body;
  }

  private url(path: string, project: Project, params: Record<string, string>): string {
    const query = new URLSearchParams({ cwd: project.cwd, ...params });
    if (project.machine) {
      query.set("machine", project.machine);
    }
    return `${this.connection.url}${path}?${query}`;
  }
}
