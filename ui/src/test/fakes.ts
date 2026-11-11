import { vi } from "vitest";
import type { GateFile, RunOptions, RunRequest, SetupReport } from "../api";
import recorded from "./recorded-stream.json";

/** The events the engine's `serve` sent over a scripted run, recorded off the real server: each batch is what it
 * sent after the step its `after` names. The runs are 20261024-090000-beef00, finished and approved with a pull
 * request when the stream opens, and 20261024-140000-c0ffee, which goes from its interview to the review, blocking
 * once, and whose spec.md is edited after. The e2e test `app_event_stream` holds the file to what serve sends now,
 * and `E2E_RECORD=1 cargo test -p octoflow-e2e --test e2e app_event_stream` records it again;
 * `engine/e2e/tests/e2e/app_recording.rs` says what of it is a stand-in for the clock's values. */
export const RECORDED: { after: string; events: ({ kind: string } & Record<string, unknown>)[] }[] = recorded;

export const CONNECTION = { url: "http://127.0.0.1:4242", token: "s3cret", project: "/home/me/proj" };

/** An EventSource the test drives; every one the app opens is in `opened`. */
export class FakeEventSource {
  static opened: FakeEventSource[] = [];
  closed = false;
  private listeners = new Map<string, ((event: MessageEvent<string>) => void)[]>();

  constructor(readonly url: string) {
    FakeEventSource.opened.push(this);
  }

  static get last(): FakeEventSource {
    const source = FakeEventSource.opened.at(-1);
    if (!source) {
      throw new Error("no EventSource was opened");
    }
    return source;
  }

  addEventListener(kind: string, listener: (event: MessageEvent<string>) => void): void {
    this.listeners.set(kind, [...(this.listeners.get(kind) ?? []), listener]);
  }

  close(): void {
    this.closed = true;
  }

  /** Dispatch an event of the kind, with data as its JSON; a bare open or error without it. */
  emit(kind: string, data?: unknown): void {
    const event = new MessageEvent(kind, { data: data === undefined ? undefined : JSON.stringify(data) });
    for (const listener of this.listeners.get(kind) ?? []) {
      listener(event);
    }
  }

  /** Dispatch the recorded batches up to and including the one after the step `through` names. */
  replay(through: string): void {
    const last = RECORDED.findIndex((batch) => batch.after === through);
    if (last < 0) {
      throw new Error(`no recorded batch after ${through}`);
    }
    for (const batch of RECORDED.slice(0, last + 1)) {
      this.replayBatch(batch);
    }
  }

  /** Dispatch the one recorded batch after the step `after` names. */
  replayOne(after: string): void {
    const batch = RECORDED.find((b) => b.after === after);
    if (!batch) {
      throw new Error(`no recorded batch after ${after}`);
    }
    this.replayBatch(batch);
  }

  private replayBatch(batch: (typeof RECORDED)[number]): void {
    for (const { kind, ...data } of batch.events) {
      this.emit(kind, data);
    }
  }
}

/** The setup a stubbed server answers: whether the marker is there (`checked`, true unless given, so that the
 * dialog does not open by itself in a test that does not ask for it), the report /setup gives, and the error that
 * POST /setup/checked fails with, if it fails; `marked` counts the POSTs that wrote the marker. */
export interface FakeSetup {
  checked?: boolean;
  report?: SetupReport;
  /** A body /setup answers with 200 in place of report's JSON, such as one that is not JSON. */
  reportText?: string;
  markError?: string;
  marked?: number;
  /** What POST /setup/gate answers: these files (none unless given), or a 502 with gateError when given. */
  gateFiles?: GateFile[];
  gateError?: string;
  /** The query's cwd of each POST /setup/gate, in order. */
  gateCwds?: (string | null)[];
}

/** The workflows a stubbed server offers unless told otherwise: `default`, `tdd`, and `broken`, which does not load.
 * `default` and `tdd` share the role `build`, each with its own model for it. */
export const OPTIONS: RunOptions = {
  workflows: [
    {
      name: "default",
      description: "Spec, build and review.",
      roles: [
        { role: "build", label: "Builder", agent: "claude", model: null },
        { role: "review", label: "Reviewer", agent: "codex", model: "o3" },
      ],
    },
    {
      name: "tdd",
      description: "Tests first, then the code.",
      roles: [
        { role: "test", label: "Test Writer", agent: "claude", model: null },
        { role: "build", label: "Builder", agent: "claude", model: "sonnet" },
      ],
    },
    { name: "broken", error: "workflow broken: no steps" },
  ],
  agents: ["claude", "codex", "gemini", "opencode", "pi"],
};

/** What a stubbed server's /workflows and POST /run answer: `options` (OPTIONS unless given), or a 500 with
 * `optionsError`; and the status and body of POST /run (201 {"run": "20261024-140000-c0ffee"} unless given), sent
 * once `held`, if given, resolves. `requests` gets the query's cwd and the body of each POST /run, in order. */
export interface FakeStart {
  options?: RunOptions;
  optionsError?: string;
  runStatus?: number;
  runBody?: unknown;
  held?: Promise<void>;
  requests?: { cwd: string | null; body: RunRequest }[];
}

/** Stub EventSource, and fetch with a server whose /file answers the text of `files` by name, else 404; whose /agent
 * answers the screen of `agents` by role, else 404 as for an agent that has exited; whose POST /agent/input takes
 * any input; whose /setup endpoints answer as `setup` says, a POST of /setup/checked setting its `checked`, and a
 * POST of /setup/gate recording its cwd; and
 * whose /workflows and POST /run answer as `start` says. */
export function stubServer(
  files: Record<string, string>,
  agents: Record<string, string> = {},
  setup: FakeSetup = {},
  start: FakeStart = {},
) {
  FakeEventSource.opened = [];
  vi.stubGlobal("EventSource", FakeEventSource);
  const answer = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });
  const fetch = vi.fn(async (url: string, init?: RequestInit) => {
    const { pathname, searchParams } = new URL(url);
    const role = searchParams.get("role") ?? "";
    if (pathname === "/setup") {
      if (setup.reportText !== undefined) {
        return new Response(setup.reportText, { status: 200 });
      }
      return answer(200, setup.report ?? { checks: [], passed: true });
    }
    if (pathname === "/setup/gate" && init?.method === "POST") {
      setup.gateCwds = [...(setup.gateCwds ?? []), searchParams.get("cwd")];
      return setup.gateError === undefined ? answer(200, { files: setup.gateFiles ?? [] }) : answer(502, { error: setup.gateError });
    }
    if (pathname === "/setup/checked" && init?.method === "POST") {
      if (setup.markError !== undefined) {
        return answer(500, { error: setup.markError });
      }
      setup.checked = true;
      setup.marked = (setup.marked ?? 0) + 1;
      return answer(200, { checked: true });
    }
    if (pathname === "/setup/checked") {
      return answer(200, { checked: setup.checked ?? true });
    }
    if (pathname === "/workflows") {
      return start.optionsError === undefined ? answer(200, start.options ?? OPTIONS) : answer(500, { error: start.optionsError });
    }
    if (pathname === "/run" && init?.method === "POST") {
      start.requests = [...(start.requests ?? []), { cwd: searchParams.get("cwd"), body: JSON.parse(String(init.body)) as RunRequest }];
      await start.held;
      return answer(start.runStatus ?? 201, start.runBody ?? { run: "20261024-140000-c0ffee" });
    }
    if (pathname === "/agent/input") {
      return init?.method === "POST" ? answer(200, { run: "r", role }) : answer(501, {});
    }
    if (pathname === "/agent") {
      const text = agents[role];
      return text === undefined
        ? answer(404, { error: `the ${role} agent has exited` })
        : answer(200, { run: "r", role, text });
    }
    const name = searchParams.get("name") ?? "";
    const text = files[name];
    return text === undefined ? answer(404, { error: `no file ${name}` }) : answer(200, { run: "r", name, text });
  });
  vi.stubGlobal("fetch", fetch);
  return fetch;
}
