// The protocol of `octoflow serve`: its run records, file lists, events and agents, and the setup checks. The engine's RunServer
// (engine/src/serve.rs) and RunWatch document the endpoints and the events, and run_record every key of a record.

/** What the Tauri shell's `server` command gives: where the sidecar's server listens and its token, from `serve`'s
 * first line, the project to show first, and the release version. */
export interface Connection {
  url: string;
  token: string;
  /** The project to show first: the checkout a development build of the app was built in; null for a release's
   * app, which belongs to none. */
  project: string | null;
  /** The release version, as `octoflow --version` prints it: the engine's, which the app links. */
  version: string;
}

/** A project as the server takes it: a directory, on the saved herdr machine or, with machine empty, here. */
export interface Project {
  cwd: string;
  machine: string;
}

/** One of the workflow's steps as the run record gives it. role_label, loop_to and quality_gated are absent from a
 * server before 0.71.0. */
export interface Step {
  id: string;
  role: string;
  /** The workflow's label for the role, as its pane shows it. */
  role_label?: string;
  /** The step a CHANGES_REQUESTED sends the run back to; null for a step that gives no verdict. */
  loop_to?: string | null;
  /** true when the quality loop follows, or followed, this step's turns in this run. */
  quality_gated?: boolean;
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

/** One handoff file's turn as the run saved it (the engine's Workflow::turn_record); null means not known, or not
 * yet. `recording.test.ts` holds the keys to what the server sends, but not those of tokens' counts, of which the
 * recording has no example. */
export interface Turn {
  step: string;
  role: string;
  round: number;
  /** The quality round the turn belongs to; 0 for a step the quality gate does not judge. */
  quality_round: number;
  /** The handoff file the turn writes, by its name in the run directory. */
  file: string;
  /** The harness, model and session the turn's agent ran in; all null for a quality analysis, which runs no agent,
   * model also for the harness's own, and session for a turn that ended in another session than it started in. */
  agent: string | null;
  model: string | null;
  session: string | null;
  /** The tokens of the answers the turn's agent gave, by kind (the engine's Tokens::to_json names the kinds), counted
   * once the run is done; "unknown" when the turn's session has no usage to count; null until the run is done, and
   * always for a quality analysis. */
  tokens: Record<string, number> | "unknown" | null;
  /** true for a turn whose length a human decides: an interview, or a wait for the answer to a BLOCKED report. */
  human_paced: boolean;
  started_at: string | null;
  ended_at: string | null;
  /** From started_at to ended_at; null while either is. */
  seconds: number | null;
}

/** One verdict a verdict step gave: the round it judged, the step's id, and APPROVE or CHANGES_REQUESTED. */
export interface StepVerdict {
  round: number;
  step: string;
  verdict: string;
}

/** One run as `list --json` prints it (run_record in the engine's cli/listing.rs, which says what each key holds);
 * null means the key does not apply. `recording.test.ts` holds the keys to what the server sends. */
export interface RunRecord {
  id: string;
  key: string;
  task: string;
  workflow: string;
  steps: Step[] | null;
  /** Whether the run opens a pull request; absent from a server before 0.71.0. */
  pull_request?: boolean;
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
  /** Each verdict a verdict step gave, in the order given; absent from a server before 0.58.0. */
  verdicts?: StepVerdict[];
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

/** One thing a run needs, as `octoflow doctor` checks it: `required` for any run, `needed-for-pr` for a run
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

/** A file the quality gate's setup looked after: its path from the project's top level, and whether the setup wrote
 * it; false means something was there already, which the setup kept as it was. */
export interface GateFile {
  path: string;
  written: boolean;
}

/** The workflow a run takes unless another is chosen, as `run` takes it without --workflow, and the one a new
 * workflow starts as a copy of. */
export const DEFAULT_WORKFLOW = "default";

/** A role of a workflow as /workflows offers it: the harness it runs in unless the run names another, and its
 * workflow's model for it, null for the harness's own. */
export interface OfferedRole {
  role: string;
  label: string;
  agent: string;
  model: string | null;
}

/** A workflow a run can be started with, by the name `run --workflow` takes; one whose file does not load has the
 * error `run --workflow` would give in place of its description and roles. description is empty when the workflow
 * has none. */
export type OfferedWorkflow = { name: string; description: string; roles: OfferedRole[] } | { name: string; error: string };

/** What a run can be started with: the workflows, in the order `octoflow workflows` lists them, and the
 * harnesses an agent can run in. */
export interface RunOptions {
  workflows: OfferedWorkflow[];
  agents: string[];
}

/** A run to start, as POST /run takes it, each field the `run` flag or argument of its name. A field left out is
 * left to `run`, as a flag not given is: workflow `default`, and the workflow's agents and models. role_agents and
 * role_models map a role of the workflow, by its key, to what its --role-agent and --role-model give it. */
export interface RunRequest {
  task: string;
  workflow?: string;
  agent?: string;
  model?: string;
  role_agents?: Record<string, string>;
  role_models?: Record<string, string>;
  no_pr: boolean;
  worktree: boolean;
}

/** A run POST /run started: its id, and the named herdr session its workspace is in, which
 * `herdr session attach <herdrSession>` shows; null when it is in the herdr server the sidecar's environment
 * reaches, which the user's own herdr shows. */
export interface StartedRun {
  run: string;
  herdrSession: string | null;
}

/** One run a sweep of POST /close looked at, as `close --all` would: its outcome "closed" (for a dry run: would be
 * closed), "already closed" (closed before; it stays so), "skipped" (it must not be closed now, such as one still
 * running) or "failed". detail is what was or would be done, what was recorded when it was closed before, or why it
 * was skipped or failed. discarded is the uncommitted changes, one line of `git status --short` each, that went or
 * would go with a failed run's worktree, a run whose close failed after removing it too; empty for any other run. */
export interface ClosedRun {
  run: string;
  outcome: "closed" | "already closed" | "skipped" | "failed";
  detail: string;
  discarded: string[];
}

/** One run a sweep of POST /forget looked at, as `forget --all` would: its outcome "forgotten" (for a dry run: would
 * be; its directory deleted, its metrics kept), "not closed" (an open run; it stays), "waiting for prune" (closed, but
 * prune has yet to delete its branch, which needs the run's state), "skipped" (its state cannot be read, or a listed run
 * is gone) or "failed". detail says why for "waiting for prune", "skipped" and "failed"; empty for the others. */
export interface ForgottenRun {
  run: string;
  outcome: "forgotten" | "not closed" | "waiting for prune" | "skipped" | "failed";
  detail: string;
}

/** A role of a workflow definition, keyed by its role key in WorkflowDefinition.roles: the label its pane shows, and
 * the harness and model it runs in unless a run names others; agent and model absent for the run's, else the
 * built-in harness's and that harness's own model. */
export interface RoleDefinition {
  label: string;
  agent?: string;
  model?: string;
}

/** A step of a workflow definition, each field the engine's Step field of its name (engine/src/pipeline.rs says what
 * each holds). A field absent is its default: again and fresh_note not set, loop_to none (a step that gives no
 * verdict), each flag false. */
export interface StepDefinition {
  id: string;
  role: string;
  file: string;
  prompt: string;
  again?: string;
  fresh_note?: string;
  fresh_repeats_again?: boolean;
  human_paced?: boolean;
  edits?: boolean;
  loop_to?: string;
  quality_gated?: boolean;
  locks?: boolean;
}

/** The step flags, which a definition leaves out where false, in the order the editor shows them. Each has its
 * explanation in STEP_FLAG_HELP. */
export const STEP_FLAGS = ["fresh_repeats_again", "human_paced", "edits", "quality_gated", "locks"] as const;

/** What each step flag does, in one sentence for the editor's user: its checkbox's tooltip and accessible
 * description. Written from the engine's Step fields (engine/src/pipeline.rs) and kept in step with the README's
 * table of step keys. */
export const STEP_FLAG_HELP: Record<(typeof STEP_FLAGS)[number], string> = {
  fresh_repeats_again: 'In a later round, a fresh session also gets the "again" prompt, not just the first one.',
  human_paced: "The step waits on you: no turn timeout or stall notice, and its pane gets focus when ready.",
  edits: "The step changes the code; the run's base and branch are fixed just before the first such step.",
  quality_gated: "With --quality-gate, the gate runs after each turn and sends a failing result back to this step.",
  locks: "Files this step changes are locked, and another step's later changes to them undone; only the first editing step may lock.",
};

/** A workflow's definition as a workflow file holds it, in JSON: the keys the engine's WORKFLOW_KEYS
 * (engine/src/workflow_file.rs) lists. A key absent is its default: no description, the engine's diff budget, no
 * proof command. */
export interface WorkflowDefinition {
  description?: string;
  max_files?: number;
  max_lines?: number;
  proof_command?: string;
  roles: Record<string, RoleDefinition>;
  steps: StepDefinition[];
}

/** A workflow file as GET /workflow read it: its path, its text, which a save or delete of it takes as its base, and
 * whether that text was written by hand rather than by a save, so that saving rewrites it without its comments and
 * with its `use` lines resolved. */
export interface WorkflowFile {
  path: string;
  text: string;
  handWritten: boolean;
}

/** A workflow as an editor opens it: its definition, canonical, each `use` resolved with the built-in prompts, and
 * its file, null for a built-in workflow; or, for a file that does not load, why, with its path and text (null when
 * the file cannot be read), the base a delete of it takes. */
export type EditableWorkflow =
  | { name: string; definition: WorkflowDefinition; file: WorkflowFile | null }
  | { name: string; error: string; path: string; text: string | null };

/** What the server finds in a definition under a name: the first problem a save would refuse, null for none, and the
 * placeholders its prompts may use, `task` and the like first, then each step's paths. */
export interface WorkflowCheck {
  problem: string | null;
  placeholders: string[];
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

  /** Write, at the top level of the project's checkout where its agents run, what `--quality-gate local` reads,
   * where it is missing: ci/gate.pin and a starter gate.toml, whose test commands are guessed from the project's
   * kind. Nothing is committed, and a run's gate reads them from its base commit. Resolves to each file, the pin
   * first; rejects with the server's error message, status 502 for a project it cannot reach or a setup that fails,
   * as for a directory in no git checkout. */
  async setUpGate(project: Project): Promise<GateFile[]> {
    const body = (await this.json(this.url("/setup/gate", project, {}), { method: "POST" })) as { files?: unknown };
    if (!Array.isArray(body.files)) {
      throw new HttpError(200, "the answer has no files");
    }
    return body.files as GateFile[];
  }

  /** What a run can be started with, from the configuration of this computer, where the server starts its runs, so
   * the same for every project; rejects with the server's error when its workflows cannot be listed. */
  async workflows(): Promise<RunOptions> {
    return this.json(`${this.connection.url}/workflows`) as Promise<RunOptions>;
  }

  /** The workflow of that name, to edit, from this computer's configuration as workflows() lists it. Rejects with the
   * server's error message, status 404 for no such workflow and 500 for a workflows directory that cannot be
   * listed. */
  async workflow(name: string): Promise<EditableWorkflow> {
    const url = `${this.connection.url}/workflow?${new URLSearchParams({ name })}`;
    const { status, body } = await this.request(url);
    // An answer that is not JSON reads as one with no keys.
    const answer = (body ?? {}) as {
      definition?: WorkflowDefinition;
      path?: string | null;
      text?: string | null;
      hand_written?: boolean;
      error?: string;
    };
    if (status === 422 && typeof answer.error === "string" && typeof answer.path === "string") {
      return { name, error: answer.error, path: answer.path, text: answer.text ?? null };
    }
    if (status < 200 || status > 299) {
      throw new HttpError(status, answer.error ?? `HTTP ${status}`);
    }
    if (typeof answer.definition !== "object" || answer.definition === null) {
      throw new HttpError(status, "the answer has no definition");
    }
    const file =
      typeof answer.path === "string" && typeof answer.text === "string"
        ? { path: answer.path, text: answer.text, handWritten: answer.hand_written === true }
        : null;
    return { name, definition: answer.definition, file };
  }

  /** What saving definition as workflow name would refuse, and the placeholders its prompts may use, as `run` would
   * read the file; nothing is written. Rejects with the server's error message. */
  async checkWorkflow(name: string, definition: WorkflowDefinition): Promise<WorkflowCheck> {
    return this.post("/workflow/check", { name, definition }) as Promise<WorkflowCheck>;
  }

  /** Write definition as workflow name's file, in the canonical form a save writes, and resolve to its path. base is
   * the file's text the editor opened (WorkflowFile.text), null for a workflow that has no file yet. Rejects with the
   * server's error message, nothing written: status 409 when the file is not base (one exists under a new name, or it
   * changed or went since it was opened), 422 for a problem checkWorkflow would name, 500 for a failed write. */
  async saveWorkflow(name: string, definition: WorkflowDefinition, base: string | null): Promise<string> {
    return this.path(await this.post("/workflow/save", { name, definition, base }));
  }

  /** Remove workflow name's file, base being its text as it was read (WorkflowFile.text, or that of a file that does
   * not load), and resolve to its path. Runs already started with it keep their own copy. Rejects with the server's
   * error message, nothing removed: status 400 for a built-in workflow, 404 for none of that name, 409 when the file
   * is no longer base. */
  async deleteWorkflow(name: string, base: string): Promise<string> {
    return this.path(await this.post("/workflow/delete", { name, base }));
  }

  /** Start the run request asks for in project, in a new workspace of this computer's herdr, and resolve to
   * the run started (StartedRun) once it has saved its state or the server's 30 seconds of waiting for that have
   * passed, whichever is first: the run may not be in the event stream yet, and one that exits after the wait never
   * will be. Rejects with
   * the server's error message: status 400 for a request it refuses, 413 for one too long, 422 for a run that exited
   * before saving its state, the message ending with the last lines its pane showed, 500 when the server cannot tell
   * its own program or make a run id, and 502 for a project it cannot reach or a run herdr cannot start. */
  async startRun(project: Project, request: RunRequest): Promise<StartedRun> {
    const body = (await this.json(this.url("/run", project, {}), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
    })) as { run?: unknown; herdr_session?: unknown };
    if (typeof body.run !== "string") {
      throw new HttpError(200, "the answer has no run");
    }
    return { run: body.run, herdrSession: typeof body.herdr_session === "string" ? body.herdr_session : null };
  }

  /** What closing each run of the project that `close --all` closes would do, as its dry run says, changing
   * nothing. Resolves to each run, in run id order. Rejects with the server's error message, status 502 for a project
   * it cannot reach or whose runs cannot be read. */
  previewClose(project: Project): Promise<ClosedRun[]> {
    return this.sweep("/close", project, { dry_run: true });
  }

  /** Close the runs of preview, a previewClose of the project, whose outcome is "closed", as `close` does: each one's
   * workspace and refs, and a failed run's worktree with its branch when that has no commit of its own. A run is
   * closed only while closing it would still do what preview says, discarding the same changes; one that changed
   * since is skipped (Sweep::Previewed in engine/src/cli/close.rs says how closely it is compared). No other run is
   * touched.
   * Resolves to each of those runs, in preview's order; a run that fails does not stop the others. Rejects as
   * previewClose does. */
  closeRuns(project: Project, preview: ClosedRun[]): Promise<ClosedRun[]> {
    const close = preview.filter((r) => r.outcome === "closed").map(({ run, detail, discarded }) => ({ run, detail, discarded }));
    return this.sweep("/close", project, { close });
  }

  /** What forgetting each run of the project would do, as `forget --all --dry-run` says, changing nothing. Resolves to
   * each run, in run id order. Rejects with the server's error message, status 502 for a project it cannot reach or
   * whose runs cannot be read. */
  previewForget(project: Project): Promise<ForgottenRun[]> {
    return this.sweep("/forget", project, { dry_run: true });
  }

  /** Forget the runs of preview, a previewForget of the project, whose outcome is "forgotten": delete each one's
   * directory, keeping its metrics, as `forget` does. A run is forgotten only while it is still one forget may delete;
   * no other run is touched. Resolves to each of those runs, in preview's order; a run that fails does not stop the
   * others. Rejects as previewForget does. */
  forgetRuns(project: Project, preview: ForgottenRun[]): Promise<ForgottenRun[]> {
    const forget = preview.filter((r) => r.outcome === "forgotten").map((r) => r.run);
    return this.sweep("/forget", project, { forget });
  }

  /** The runs of a POST of body to path, /close or /forget, for the project, as the methods that sweep through it
   * resolve to them. */
  private async sweep<Run>(path: "/close" | "/forget", project: Project, body: unknown): Promise<Run[]> {
    const answer = (await this.json(this.url(path, project, {}), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    })) as { runs?: unknown };
    if (!Array.isArray(answer.runs)) {
      throw new HttpError(200, "the answer has no runs");
    }
    return answer.runs as Run[];
  }

  /** The text field of a GET of url's JSON; rejects with the server's error message, or the status. */
  private async text(url: string): Promise<string> {
    const body = (await this.json(url)) as { text?: string };
    if (body.text === undefined) {
      throw new HttpError(200, "the answer has no text");
    }
    return body.text;
  }

  /** The JSON answer to a POST of body, as JSON, to the server's path, which names no project; rejects as json does. */
  private async post(path: string, body: unknown): Promise<unknown> {
    return this.json(`${this.connection.url}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  }

  /** The path field of a workflow file's save or delete answer. */
  private path(answer: unknown): string {
    const { path } = answer as { path?: unknown };
    if (typeof path !== "string") {
      throw new HttpError(200, "the answer has no path");
    }
    return path;
  }

  /** The JSON body of a request of url, with the token and init; rejects with an HttpError carrying the server's
   * error message, or the status, for any answer but a 2xx, and for a 2xx whose body is not JSON. */
  private async json(url: string, init: RequestInit = {}): Promise<unknown> {
    const { status, body } = await this.request(url, init);
    const answer = body as { error?: string } | undefined;
    if (status < 200 || status > 299) {
      throw new HttpError(status, answer?.error ?? `HTTP ${status}`);
    }
    if (body === undefined) {
      throw new HttpError(status, "the answer is not JSON");
    }
    return body;
  }

  /** The status and JSON body of a request of url, with the token and init, whatever the status; body undefined when
   * it is not JSON, which an error answer may be. Rejects only when no answer comes. */
  private async request(url: string, init: RequestInit = {}): Promise<{ status: number; body: unknown }> {
    const headers = { ...init.headers, Authorization: `Bearer ${this.connection.token}` };
    const response = await fetch(url, { ...init, headers });
    return { status: response.status, body: await response.json().catch(() => undefined) };
  }

  private url(path: string, project: Project, params: Record<string, string>): string {
    const query = new URLSearchParams({ cwd: project.cwd, ...params });
    if (project.machine) {
      query.set("machine", project.machine);
    }
    return `${this.connection.url}${path}?${query}`;
  }
}
