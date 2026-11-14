import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { AGENT_POLL_MS } from "./components/AgentPanes";
import { CONNECTION, FakeEventSource, OPTIONS, RECORDED, stubServer, type FakeSetup, type FakeStart } from "./test/fakes";

const RUN = "20261024-140000-c0ffee";
const DONE = "20261024-090000-beef00";
const connect = () => Promise.resolve(CONNECTION);

const SPEC = "# Add a token-bucket rate limiter\n\n- **10** requests a second\n\n| Limit | Value |\n|---|---|\n| burst | 20 |\n\n<script>alert('x')</script>\n<b>raw</b>\n";

async function openApp(files: Record<string, string> = {}, agents: Record<string, string> = {}) {
  const fetch = stubServer(files, agents);
  render(<App connect={connect} />);
  // The stream opens in an effect, which may run after the first render shows.
  await waitFor(() => expect(FakeEventSource.opened).toHaveLength(1));
  return { fetch, source: FakeEventSource.last };
}

function row(run: string): HTMLElement {
  const element = document.querySelector<HTMLElement>(`tr[data-run="${run}"]`);
  if (!element) {
    throw new Error(`no row for ${run}`);
  }
  return element;
}

function stages(container: HTMLElement): [string | null, string | null][] {
  return [...container.querySelectorAll("li.stage")].map((li) => [li.textContent, li.getAttribute("data-state")]);
}

/** The run map's stations in the run detail, each as [stage name, state]. */
function stations(detail: HTMLElement): [string | null, string | null][] {
  const map = within(detail).getByRole("list", { name: "stages" });
  return within(map).getAllByRole("listitem").map((g) => [g.querySelector(".label")?.textContent ?? null, g.getAttribute("data-state")]);
}

describe("the runs overview", () => {
  it("opens the checkout's project on the server's stream, with the token", async () => {
    const { source } = await openApp();
    const url = new URL(source.url);
    expect(`${url.origin}${url.pathname}`).toBe("http://127.0.0.1:4242/events");
    expect(url.searchParams.get("cwd")).toBe("/home/me/proj");
    expect(url.searchParams.get("token")).toBe("s3cret");
    expect(url.searchParams.has("machine")).toBe(false);
    expect(screen.getByText("No runs.")).toBeInTheDocument();
  });

  it("follows a run through its phases as the events arrive", async () => {
    const { source } = await openApp();
    act(() => source.replay("the stream opened"));
    const done = row(DONE);
    expect(within(done).getByText("APPROVE")).toBeInTheDocument();
    expect(within(done).getByRole("link", { name: "#1" })).toHaveAttribute("href", "https://github.com/parity/project/pull/1");
    expect(stages(done)).toEqual([["spec", "done"], ["build", "done"], ["review", "done"], ["publish", "done"]]);

    act(() => source.replayOne("a run started its interview"));
    expect(stages(row(RUN))).toEqual([["spec", "current"], ["build", "todo"], ["review", "todo"], ["publish", "todo"]]);
    expect(within(row(RUN)).getByText("round 0")).toBeInTheDocument();
    expect(within(row(RUN)).getByText(/^running, beat \d+s ago$/)).toBeInTheDocument();
    // Newest first.
    expect([...document.querySelectorAll("tbody tr")].map((tr) => tr.getAttribute("data-run"))).toEqual([RUN, DONE]);

    act(() => source.replayOne("the spec was written and the build started"));
    expect(stages(row(RUN))).toEqual([["spec", "done"], ["build", "current"], ["review", "todo"], ["publish", "todo"]]);
    expect(within(row(RUN)).getByText("round 1")).toBeInTheDocument();
    expect(row(RUN).querySelector(".role")).toHaveTextContent(/^build$/);

    act(() => source.replayOne("the Builder blocked"));
    expect(within(row(RUN)).getByText("blocked: should the limit be per client or global?")).toBeInTheDocument();

    act(() => source.replayOne("the build was reported and the review started"));
    expect(stages(row(RUN))).toEqual([["spec", "done"], ["build", "done"], ["review", "current"], ["publish", "todo"]]);
    expect(within(row(RUN)).queryByText(/blocked/)).not.toBeInTheDocument();

    act(() => source.replay("the finished run was deleted"));
    expect(document.querySelector(`tr[data-run="${DONE}"]`)).toBeNull();
    expect(row(RUN)).toBeInTheDocument();
  });

  it("ages the heartbeat between events", async () => {
    // Frozen time, which moves only when advanced: findBy's polling would need it to move by itself.
    vi.useFakeTimers();
    stubServer({});
    render(<App connect={connect} />);
    await act(async () => {});
    const source = FakeEventSource.last;
    act(() => source.replay("a run started its interview"));
    const age = () => Number(within(row(RUN)).getByText(/^running, beat/).textContent?.match(/(\d+)s/)?.[1]);
    const before = age();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(age()).toBe(before + 3);
  });

  it("shows the server's problem until it clears", async () => {
    const { source } = await openApp();
    act(() => source.emit("problem", { error: "ssh: connection refused" }));
    expect(screen.getByText("ssh: connection refused")).toBeInTheDocument();
    act(() => source.emit("problem", { error: null }));
    expect(screen.queryByText("ssh: connection refused")).not.toBeInTheDocument();
  });

  it("starts from no runs when the stream reconnects", async () => {
    const { source } = await openApp();
    act(() => source.replay("the stream opened"));
    act(() => source.emit("open"));
    expect(screen.getByText("No runs.")).toBeInTheDocument();
  });

  it("opens another project, on a machine, and remembers it", async () => {
    const { source } = await openApp();
    fireEvent.change(screen.getByLabelText("Project folder"), { target: { value: " ~/other " } });
    fireEvent.change(screen.getByLabelText("Machine"), { target: { value: "devbox" } });
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(source.closed).toBe(true);
    const url = new URL(FakeEventSource.last.url);
    expect(url.searchParams.get("cwd")).toBe("~/other");
    expect(url.searchParams.get("machine")).toBe("devbox");
    expect(JSON.parse(localStorage.getItem("orchestrator.project") ?? "")).toEqual({ cwd: "~/other", machine: "devbox" });
  });

  it("asks for a project when the orchestrator belongs to no checkout", async () => {
    stubServer({});
    render(<App connect={() => Promise.resolve({ ...CONNECTION, project: null })} />);
    expect(await screen.findByText("Open a project folder to see its runs.")).toBeInTheDocument();
    expect(FakeEventSource.opened).toHaveLength(0);
    fireEvent.change(screen.getByLabelText("Project folder"), { target: { value: "~/proj" } });
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(new URL(FakeEventSource.last.url).searchParams.get("cwd")).toBe("~/proj");
  });

  it("says why the orchestrator could not start", async () => {
    render(<App connect={() => Promise.reject(new Error("sidecar: not found"))} />);
    expect(await screen.findByText(/Could not start the orchestrator: Error: sidecar: not found/)).toBeInTheDocument();
  });
});

describe("the run detail", () => {
  it("renders the newest handoff file as Markdown, without its raw HTML", async () => {
    const { fetch, source } = await openApp({ "spec.md": SPEC });
    act(() => source.replay("the spec was written and the build started"));
    fireEvent.click(row(RUN));
    const detail = screen.getByRole("region", { name: "run c0ffee" });
    expect(stations(detail)).toEqual([["spec", "done"], ["build", "current"], ["review", "todo"], ["publish", "todo"]]);
    expect(await within(detail).findByRole("heading", { name: "Add a token-bucket rate limiter" })).toBeInTheDocument();
    expect(within(detail).getByText("10").tagName).toBe("STRONG");
    expect(within(detail).getByRole("cell", { name: "20" })).toBeInTheDocument();
    expect(detail.querySelector("article script, article b")).toBeNull();
    expect(within(detail).queryByText(/alert/)).not.toBeInTheDocument();
    const calls = fetch.mock.calls as unknown as [string, RequestInit][];
    const [url, init] = calls.find(([u]) => new URL(u).pathname === "/file") ?? ["", {}];
    expect(new URL(url).searchParams.get("name")).toBe("spec.md");
    expect(new URL(url).searchParams.get("run")).toBe(RUN);
    expect(init.headers).toEqual({ Authorization: "Bearer s3cret" });
  });

  it("lists new files as they appear and renders the chosen one again when it changes", async () => {
    const files = { "spec.md": SPEC, "build-1.blocked-1.md": "BLOCKED: per client?\n", "build-1.md": "# Build 1\n" };
    const { fetch, source } = await openApp(files);
    act(() => source.replay("the spec was written and the build started"));
    fireEvent.click(row(RUN));
    const detail = screen.getByRole("region", { name: "run c0ffee" });
    const names = () => within(within(detail).getByRole("list", { name: "files" })).getAllByRole("button").map((b) => b.firstChild?.textContent);
    expect(names()).toEqual(["spec.md"]);
    await within(detail).findByRole("heading", { name: "Add a token-bucket rate limiter" });
    fireEvent.click(within(detail).getByRole("button", { name: /spec\.md/ }));

    act(() => source.replay("the build was reported and the review started"));
    expect(names()).toEqual(["spec.md", "build-1.blocked-1.md", "build-1.md"]);
    expect(stations(detail)).toEqual([["spec", "done"], ["build", "done"], ["review", "current"], ["publish", "todo"]]);
    // The chosen file stays, and is not fetched again while it is unchanged.
    expect(within(detail).getByRole("heading", { name: "Add a token-bucket rate limiter" })).toBeInTheDocument();
    const fetches = fetch.mock.calls.length;

    files["spec.md"] = "# Add a token-bucket rate limiter\n\n- **20** requests a second\n";
    act(() => source.replayOne("the spec was edited"));
    expect(await within(detail).findByText("20")).toBeInTheDocument();
    expect(fetch.mock.calls.length).toBe(fetches + 1);

    fireEvent.click(within(detail).getByRole("button", { name: /build-1\.md/ }));
    expect(await within(detail).findByRole("heading", { name: "Build 1" })).toBeInTheDocument();
  });

  it("says when a file cannot be read", async () => {
    const { source } = await openApp({});
    act(() => source.replay("the spec was written and the build started"));
    fireEvent.click(row(RUN));
    expect(await screen.findByText("no file spec.md")).toBeInTheDocument();
  });

  it("goes back to the overview", async () => {
    const { source } = await openApp({ "spec.md": SPEC });
    act(() => source.replay("the spec was written and the build started"));
    fireEvent.click(row(RUN));
    fireEvent.click(screen.getByRole("button", { name: "‹ Runs" }));
    expect(row(RUN)).toBeInTheDocument();
  });

  it("goes back to the overview when its run is deleted", async () => {
    const { source } = await openApp({ "review-1.md": "VERDICT: APPROVE\n" });
    act(() => source.replay("the stream opened"));
    fireEvent.click(row(DONE));
    expect(screen.getByRole("region", { name: "run beef00" })).toBeInTheDocument();
    act(() => source.replay("the finished run was deleted"));
    expect(screen.queryByRole("region", { name: "run beef00" })).not.toBeInTheDocument();
  });
});

describe("the agents", () => {
  const AGENTS = [
    { role: "spec", name: "spec-c0ffee", kind: "claude" },
    { role: "build", name: "build-c0ffee", kind: "codex" },
  ];

  /** Open the app on the run in its build, with the agents above, and its detail. */
  async function openAgents(screens: Record<string, string>) {
    const opened = await openApp({ "spec.md": SPEC }, screens);
    act(() => opened.source.replay("the spec was written and the build started"));
    const batch = RECORDED.find((b) => b.after === "the spec was written and the build started");
    const record = batch?.events.find((e) => e.kind === "record")?.record as Record<string, unknown>;
    act(() => opened.source.emit("record", { run: RUN, record: { ...record, agents: AGENTS } }));
    fireEvent.click(row(RUN));
    return { ...opened, agents: screen.getByRole("region", { name: "agents" }) };
  }

  function agentCalls(fetch: ReturnType<typeof stubServer>) {
    return fetch.mock.calls
      .map(([url, init]) => [new URL(url as string), init as RequestInit | undefined] as const)
      .filter(([url]) => url.pathname.startsWith("/agent"));
  }

  it("shows the current role's agent first, and another when its tab is chosen", async () => {
    const { fetch, agents } = await openAgents({ build: "> building the limiter\n", spec: "> spec written\n" });
    expect(within(agents).getAllByRole("tab").map((t) => t.textContent)).toEqual(["spec", "build"]);
    expect(within(agents).getByRole("tab", { name: "build" })).toHaveAttribute("aria-selected", "true");
    expect(await within(agents).findByText("> building the limiter")).toBeInTheDocument();
    const [url, init] = agentCalls(fetch)[0] ?? [];
    const asked = [url?.pathname, url?.searchParams.get("run"), url?.searchParams.get("role"), url?.searchParams.get("lines")];
    expect(asked).toEqual(["/agent", RUN, "build", "200"]);
    expect(init?.headers).toEqual({ Authorization: "Bearer s3cret" });

    fireEvent.click(within(agents).getByRole("tab", { name: "spec" }));
    expect(await within(agents).findByText("> spec written")).toBeInTheDocument();
  });

  it("reads the shown agent again while it is shown", async () => {
    vi.useFakeTimers();
    const fetch = stubServer({ "spec.md": SPEC }, { build: "> one\n" });
    render(<App connect={connect} />);
    await act(async () => {});
    const source = FakeEventSource.last;
    act(() => source.replay("the spec was written and the build started"));
    const batch = RECORDED.find((b) => b.after === "the spec was written and the build started");
    const record = batch?.events.find((e) => e.kind === "record")?.record as Record<string, unknown>;
    act(() => source.emit("record", { run: RUN, record: { ...record, agents: AGENTS } }));
    fireEvent.click(row(RUN));
    await act(async () => {});
    const reads = () => agentCalls(fetch).length;
    expect(reads()).toBe(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(reads()).toBe(2);
    fireEvent.click(screen.getByRole("button", { name: "‹ Runs" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10000);
    });
    expect(reads()).toBe(2);
  });

  it("types a message and keys into the agent", async () => {
    const { fetch, agents } = await openAgents({ build: "> which limit?\n" });
    await within(agents).findByText("> which limit?");
    const box = within(agents).getByLabelText("Message to the build agent");
    fireEvent.change(box, { target: { value: "per client" } });
    fireEvent.click(within(agents).getByRole("button", { name: "Send" }));
    await waitFor(() => expect(box).toHaveValue(""));
    fireEvent.click(within(agents).getByRole("button", { name: "press esc" }));
    await waitFor(() => expect(agentCalls(fetch).filter(([, init]) => init?.method === "POST")).toHaveLength(2));
    const posts = agentCalls(fetch).filter(([, init]) => init?.method === "POST");
    expect(posts.map(([url, init]) => [url.pathname, url.searchParams.get("role"), init?.body])).toEqual([
      ["/agent/input", "build", JSON.stringify({ text: "per client" })],
      ["/agent/input", "build", JSON.stringify({ keys: ["esc"] })],
    ]);
  });

  it("sends a message once while it is being sent, and keeps it when the send fails", async () => {
    const { fetch, agents } = await openAgents({ build: "> which limit?\n" });
    await within(agents).findByText("> which limit?");
    // Hold each send until the test answers it.
    const serve = fetch.getMockImplementation();
    const answers: ((response: Response) => void)[] = [];
    fetch.mockImplementation((url: string, init?: RequestInit) =>
      new URL(url).pathname === "/agent/input"
        ? new Promise<Response>((resolve) => answers.push(resolve))
        : (serve?.(url, init) ?? Promise.reject(new Error("no server"))),
    );
    const posts = () => agentCalls(fetch).filter(([, init]) => init?.method === "POST");
    const box = within(agents).getByLabelText("Message to the build agent");
    const send = within(agents).getByRole("button", { name: "Send" });
    fireEvent.change(box, { target: { value: "per client" } });
    fireEvent.submit(box);
    fireEvent.submit(box);
    fireEvent.click(send);
    expect(posts()).toHaveLength(1);
    expect(send).toBeDisabled();

    await act(async () => answers[0]?.(new Response(JSON.stringify({ error: "herdr: no such pane" }), { status: 502 })));
    expect(await within(agents).findByText("herdr: no such pane")).toBeInTheDocument();
    expect(box).toHaveValue("per client");
    expect(send).toBeEnabled();

    fireEvent.submit(box);
    expect(posts()).toHaveLength(2);
    await act(async () => answers[1]?.(new Response(JSON.stringify({ run: "r", role: "build" }), { status: 200 })));
    await waitFor(() => expect(box).toHaveValue(""));
  });

  it("stops reading an agent that has exited, until a send", async () => {
    vi.useFakeTimers();
    const fetch = stubServer({ "spec.md": SPEC }, {});
    render(<App connect={connect} />);
    await act(async () => {});
    const source = FakeEventSource.last;
    act(() => source.replay("the spec was written and the build started"));
    const batch = RECORDED.find((b) => b.after === "the spec was written and the build started");
    const record = batch?.events.find((e) => e.kind === "record")?.record as Record<string, unknown>;
    act(() => source.emit("record", { run: RUN, record: { ...record, agents: AGENTS } }));
    fireEvent.click(row(RUN));
    await act(async () => {});
    const reads = () => agentCalls(fetch).filter(([url]) => url.pathname === "/agent").length;
    expect(screen.getByText("the build agent has exited")).toBeInTheDocument();
    expect(reads()).toBe(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10 * AGENT_POLL_MS);
    });
    expect(reads()).toBe(1);

    fireEvent.click(screen.getByRole("button", { name: "press esc" }));
    await act(async () => {});
    expect(reads()).toBe(2);
  });

  it("says when an agent has exited, and when none has started", async () => {
    const { agents, source } = await openAgents({});
    expect(await within(agents).findByText("the build agent has exited")).toBeInTheDocument();

    // A run before its first agent, and one from a server before 0.51.0, whose records have no agents key: the
    // recording has neither, every run in it having started an agent.
    const done = RECORDED[0]?.events.find((e) => e.kind === "record")?.record as Record<string, unknown>;
    const keyless = Object.fromEntries(Object.entries(done).filter(([key]) => key !== "agents"));
    for (const record of [{ ...done, agents: [] }, keyless]) {
      act(() => source.emit("record", { run: DONE, record }));
      fireEvent.click(screen.getByRole("button", { name: "‹ Runs" }));
      fireEvent.click(row(DONE));
      expect(within(screen.getByRole("region", { name: "agents" })).getByText("No agent has started yet.")).toBeInTheDocument();
    }
  });
});

describe("the setup dialog", () => {
  const REPORT = {
    checks: [
      { name: "herdr", level: "required", ok: true, detail: "herdr 0.9.1", fix: "" },
      { name: "reaching herdr", level: "optional", ok: false, detail: "not inside a herdr pane", fix: "run from a herdr pane, or choose a herdr machine (--machine)" },
      { name: "gh", level: "needed-for-pr", ok: false, detail: "gh auth status fails", fix: "gh auth login" },
    ],
    passed: true,
  } as const;
  const FAILED = { checks: [{ name: "git", level: "required", ok: false, detail: "git is not on PATH", fix: "install git 2.36 or later" }], passed: false } as const;

  async function openWith(setup: FakeSetup) {
    const fetch = stubServer({}, {}, setup);
    render(<App connect={connect} />);
    await waitFor(() => expect(FakeEventSource.opened).toHaveLength(1));
    return fetch;
  }

  function setupCalls(fetch: ReturnType<typeof stubServer>) {
    return fetch.mock.calls.map(([url]) => new URL(url as string)).filter((url) => url.pathname === "/setup");
  }

  it("opens by itself without the marker, and checks the open project", async () => {
    const fetch = await openWith({ checked: false, report: { ...REPORT, checks: [...REPORT.checks] } });
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    expect(await within(dialog).findByText("herdr 0.9.1", { exact: false })).toBeInTheDocument();
    const required = within(dialog).getByRole("region", { name: "Needed for any run" });
    expect(within(required).getByText("ok")).toBeInTheDocument();
    const pr = within(dialog).getByRole("region", { name: "Needed to open a pull request" });
    expect(within(pr).getByText("missing")).toBeInTheDocument();
    expect(within(pr).getByText("fix: gh auth login")).toBeInTheDocument();
    const some = within(dialog).getByRole("region", { name: "Needed by some runs" });
    expect(within(some).getByText("warn")).toBeInTheDocument();
    const [url] = setupCalls(fetch);
    expect(url?.searchParams.get("cwd")).toBe("/home/me/proj");
  });

  it("does not open by itself with the marker", async () => {
    const fetch = await openWith({ checked: true });
    await waitFor(() => expect(fetch.mock.calls.some(([url]) => new URL(url as string).pathname === "/setup/checked")).toBe(true));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(setupCalls(fetch)).toHaveLength(0);
  });

  it("opens from Check setup…, for the open project", async () => {
    const fetch = await openWith({ checked: true, report: { ...REPORT, checks: [...REPORT.checks] } });
    fireEvent.change(screen.getByLabelText("Machine"), { target: { value: "devbox" } });
    fireEvent.click(screen.getByRole("button", { name: "Check setup…" }));
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    expect(within(dialog).getByText("/home/me/proj on this computer")).toBeInTheDocument();
    await within(dialog).findByText("herdr 0.9.1", { exact: false });
    expect(setupCalls(fetch)[0]?.searchParams.has("machine")).toBe(false);
  });

  it("writes the marker on Close when every required check passed", async () => {
    const setup: FakeSetup = { checked: false, report: { ...REPORT, checks: [...REPORT.checks] } };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("herdr 0.9.1", { exact: false });
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(setup.marked).toBe(1);
  });

  it("writes the marker once for a double click on Close", async () => {
    const setup: FakeSetup = { checked: false, report: { ...REPORT, checks: [...REPORT.checks] } };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("herdr 0.9.1", { exact: false });
    const close = within(dialog).getByRole("button", { name: "Close" });
    fireEvent.click(close);
    fireEvent.click(close);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(setup.marked).toBe(1);
  });

  it("shows an answer to /setup that is not JSON as an error", async () => {
    await openWith({ checked: false, reportText: "" });
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    expect(await within(dialog).findByText("the answer is not JSON")).toBeInTheDocument();
  });

  it("does not write the marker when a required check failed", async () => {
    const setup: FakeSetup = { checked: false, report: { ...FAILED, checks: [...FAILED.checks] } };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    expect(await within(dialog).findByText("fix: install git 2.36 or later")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(setup.marked).toBeUndefined();
  });

  it("shows a marker that cannot be written, and closes without it the next time", async () => {
    const setup: FakeSetup = { checked: false, report: { ...REPORT, checks: [...REPORT.checks] }, markError: "cannot write /cfg/setup-checked: [Errno 13] Permission denied" };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("herdr 0.9.1", { exact: false });
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(await within(dialog).findByText(/Could not record that setup was checked: cannot write \/cfg\/setup-checked/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("sets the quality gate up for the open project, and shows what it wrote and kept", async () => {
    const files = [
      { path: "ci/gate.pin", written: true },
      { path: "gate.toml", written: false },
    ];
    const setup: FakeSetup = { checked: false, report: { ...REPORT, checks: [...REPORT.checks] }, gateFiles: files };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("herdr 0.9.1", { exact: false });
    expect(within(dialog).queryByRole("region", { name: "Quality gate" })).not.toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Set up the quality gate" }));
    const gate = await within(dialog).findByRole("region", { name: "Quality gate" });
    expect(await within(gate).findByText("wrote")).toBeInTheDocument();
    expect(within(gate).getByText("ci/gate.pin")).toBeInTheDocument();
    expect(within(gate).getByText("kept")).toBeInTheDocument();
    expect(within(gate).getByText("gate.toml")).toBeInTheDocument();
    expect(within(gate).getByText(/reads these from a run's base commit/)).toBeInTheDocument();
    expect(setup.gateCwds).toEqual(["/home/me/proj"]);
  });

  it("shows why the quality gate could not be set up", async () => {
    const setup: FakeSetup = { checked: false, report: { ...REPORT, checks: [...REPORT.checks] }, gateError: "/home/me/proj is in no git checkout" };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("herdr 0.9.1", { exact: false });
    fireEvent.click(within(dialog).getByRole("button", { name: "Set up the quality gate" }));
    const gate = await within(dialog).findByRole("region", { name: "Quality gate" });
    expect(await within(gate).findByText("/home/me/proj is in no git checkout")).toBeInTheDocument();
  });

  it("checks again", async () => {
    const setup: FakeSetup = { checked: false, report: { ...FAILED, checks: [...FAILED.checks] } };
    const fetch = await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("fix: install git 2.36 or later");
    setup.report = { ...REPORT, checks: [...REPORT.checks] };
    fireEvent.click(within(dialog).getByRole("button", { name: "Check again" }));
    expect(await within(dialog).findByText("herdr 0.9.1", { exact: false })).toBeInTheDocument();
    expect(within(dialog).queryByText("fix: install git 2.36 or later")).not.toBeInTheDocument();
    expect(setupCalls(fetch)).toHaveLength(2);
  });
});

describe("the run form", () => {
  /** The app with the run form open, once the form has read the workflows and, unless `workflow` says another,
   * chosen `default`. */
  async function openForm(start: FakeStart = {}, workflow = "default") {
    stubServer({}, {}, {}, start);
    render(<App connect={connect} />);
    await waitFor(() => expect(FakeEventSource.opened).toHaveLength(1));
    fireEvent.click(screen.getByRole("button", { name: "New run…" }));
    const form = await screen.findByRole("dialog", { name: "new run" });
    await waitFor(() => expect(within(form).queryByText("Reading the workflows…")).not.toBeInTheDocument());
    expect(within(form).getByLabelText("Workflow")).toHaveValue(workflow);
    return { form, source: FakeEventSource.last };
  }

  const typeTask = (form: HTMLElement, task: string) =>
    fireEvent.change(within(form).getByLabelText("Task"), { target: { value: task } });

  it("sends the defaults, and selects the started run", async () => {
    const start: FakeStart = {};
    const { form, source } = await openForm(start);
    expect(within(form).getByText("Spec, build and review.")).toBeInTheDocument();
    expect(within(form).getByLabelText("No pull request")).not.toBeChecked();
    expect(within(form).getByLabelText("Separate worktree")).toBeChecked();
    typeTask(form, " Add a rate limiter ");
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(start.requests).toEqual([
      { cwd: "/home/me/proj", body: { task: "Add a rate limiter", workflow: "default", no_pr: false, worktree: true } },
    ]);
    act(() => source.replay("a run started its interview"));
    expect(screen.getByRole("region", { name: "run c0ffee" })).toBeInTheDocument();
  });

  it("sends the workflow, agent and model chosen and the boxes as ticked", async () => {
    const start: FakeStart = {};
    const { form } = await openForm(start);
    expect(within(form).getByRole("option", { name: "broken (does not load)" })).toBeDisabled();
    typeTask(form, "Add a rate limiter");
    fireEvent.change(within(form).getByLabelText("Workflow"), { target: { value: "tdd" } });
    expect(within(form).getByText("Tests first, then the code.")).toBeInTheDocument();
    fireEvent.change(within(form).getByLabelText("Agent"), { target: { value: "codex" } });
    fireEvent.change(within(form).getByLabelText("Model"), { target: { value: "o3" } });
    fireEvent.click(within(form).getByLabelText("No pull request"));
    fireEvent.click(within(form).getByLabelText("Separate worktree"));
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    await waitFor(() => expect(start.requests).toHaveLength(1));
    expect(start.requests?.[0]?.body).toEqual({
      task: "Add a rate limiter",
      workflow: "tdd",
      agent: "codex",
      model: "o3",
      no_pr: true,
      worktree: false,
    });
  });

  it("disables Start without a task", async () => {
    const { form } = await openForm();
    const startButton = within(form).getByRole("button", { name: "Start" });
    expect(startButton).toBeDisabled();
    typeTask(form, "   ");
    expect(startButton).toBeDisabled();
    typeTask(form, "Add a rate limiter");
    expect(startButton).toBeEnabled();
  });

  it("shows that the run is starting until the server answers", async () => {
    let release = () => {};
    const start: FakeStart = { held: new Promise<void>((resolve) => (release = resolve)) };
    const { form } = await openForm(start);
    typeTask(form, "Add a rate limiter");
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    expect(await within(form).findByText("Starting the run…")).toBeInTheDocument();
    expect(within(form).getByRole("button", { name: "Start" })).toBeDisabled();
    expect(within(form).getByRole("button", { name: "Cancel" })).toBeDisabled();
    expect(within(form).getByLabelText("Task")).toBeDisabled();
    await act(async () => release());
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("says a run is not listed yet, and opens it once it is", async () => {
    const { form, source } = await openForm({ runStatus: 202, runBody: { run: RUN, listed: false } });
    typeTask(form, "Add a rate limiter");
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    const hint = await screen.findByText(new RegExp(`^Run ${RUN} has started but is not listed yet`));
    expect(hint).toHaveTextContent(/If it never is, its herdr workspace shows why\.$/);
    act(() => source.replay("a run started its interview"));
    expect(screen.getByRole("region", { name: "run c0ffee" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "‹ Runs" }));
    expect(screen.queryByText(/is not listed yet/)).not.toBeInTheDocument();
  });

  it("names the herdr session to attach to for a run not listed yet that is in one", async () => {
    const { form } = await openForm({ runStatus: 202, runBody: { run: RUN, listed: false, herdr_session: "octoflow" } });
    typeTask(form, "Add a rate limiter");
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    const hint = await screen.findByText(new RegExp(`^Run ${RUN} has started but is not listed yet`));
    expect(hint).toHaveTextContent(/If it never is, herdr session attach octoflow shows why\.$/);
    expect(within(hint).getByText("herdr session attach octoflow").tagName).toBe("CODE");
  });

  it("stops waiting for a run that is not listed", async () => {
    const { form } = await openForm({ runStatus: 202, runBody: { run: RUN, listed: false } });
    typeTask(form, "Add a rate limiter");
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    fireEvent.click(await screen.findByRole("button", { name: "Stop waiting" }));
    expect(screen.queryByText(/is not listed yet/)).not.toBeInTheDocument();
  });

  it("starts the default workflow when the workflows cannot be read", async () => {
    const start: FakeStart = { optionsError: "cannot list /cfg/workflows: Permission denied" };
    const { form } = await openForm(start);
    expect(within(form).getByText("cannot list /cfg/workflows: Permission denied; the run takes the default workflow")).toBeInTheDocument();
    typeTask(form, "Add a rate limiter");
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    await waitFor(() => expect(start.requests).toHaveLength(1));
    expect(start.requests?.[0]?.body.workflow).toBe("default");
  });

  it("asks for a workflow when the default one does not load", async () => {
    const options = { ...OPTIONS, workflows: [{ name: "default", error: "workflow default: no steps" }, ...OPTIONS.workflows.slice(1)] };
    const { form } = await openForm({ options }, "");
    typeTask(form, "Add a rate limiter");
    expect(within(form).getByRole("button", { name: "Start" })).toBeDisabled();
    fireEvent.change(within(form).getByLabelText("Workflow"), { target: { value: "tdd" } });
    expect(within(form).getByRole("button", { name: "Start" })).toBeEnabled();
  });

  it("keeps an error in the form, with the pane's lines and the values entered", async () => {
    const error = `run ${RUN} exited before it saved its state; its pane showed:\nerror: no workflow nope\n$`;
    const { form } = await openForm({ runStatus: 422, runBody: { error, run: RUN } });
    typeTask(form, "Add a rate limiter");
    fireEvent.change(within(form).getByLabelText("Model"), { target: { value: "o3" } });
    fireEvent.click(within(form).getByLabelText("No pull request"));
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    const shown = await within(form).findByText(/exited before it saved its state/);
    expect(shown.textContent).toBe(error);
    expect(within(form).getByLabelText("Task")).toHaveValue("Add a rate limiter");
    expect(within(form).getByLabelText("Model")).toHaveValue("o3");
    expect(within(form).getByLabelText("No pull request")).toBeChecked();
    expect(within(form).getByRole("button", { name: "Start" })).toBeEnabled();
  });

  it("closes on Cancel without starting a run", async () => {
    const start: FakeStart = {};
    const { form } = await openForm(start);
    typeTask(form, "Add a rate limiter");
    fireEvent.click(within(form).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(start.requests).toBeUndefined();
  });

  /** The labels of the rows of roles, in order. */
  const roleRows = (form: HTMLElement) =>
    within(form)
      .queryAllByRole("group", { name: /^role / })
      .map((row) => row.getAttribute("aria-label"));

  it("has a row for each role of the workflow, in its order, that follows the workflow", async () => {
    const { form } = await openForm();
    expect(roleRows(form)).toEqual(["role Builder", "role Reviewer"]);
    fireEvent.change(within(form).getByLabelText("Workflow"), { target: { value: "tdd" } });
    expect(roleRows(form)).toEqual(["role Test Writer", "role Builder"]);
  });

  it("shows a role's default agent and model, unless the run's replace them", async () => {
    const { form } = await openForm();
    const reviewerAgent = within(form).getByLabelText("Reviewer agent");
    expect(reviewerAgent).toHaveValue("");
    expect(within(reviewerAgent).getByRole("option", { selected: true })).toHaveTextContent("codex (default)");
    expect(within(form).getByLabelText("Reviewer model")).toHaveAttribute("placeholder", "o3 (default)");
    expect(within(form).getByLabelText("Builder model")).toHaveAttribute("placeholder", "the harness's own");
    fireEvent.change(within(form).getByLabelText("Agent"), { target: { value: "pi" } });
    fireEvent.change(within(form).getByLabelText("Model"), { target: { value: "m1" } });
    expect(within(reviewerAgent).getByRole("option", { selected: true })).toHaveTextContent("pi (the run's)");
    expect(within(form).getByLabelText("Reviewer model")).toHaveAttribute("placeholder", "m1 (the run's)");
  });

  it("keeps a role's values across a change of workflow that shares the role, and only that role's", async () => {
    const { form } = await openForm();
    fireEvent.change(within(form).getByLabelText("Builder agent"), { target: { value: "agy" } });
    fireEvent.change(within(form).getByLabelText("Builder model"), { target: { value: "g2" } });
    fireEvent.change(within(form).getByLabelText("Reviewer agent"), { target: { value: "pi" } });
    fireEvent.change(within(form).getByLabelText("Workflow"), { target: { value: "tdd" } });
    expect(within(form).getByLabelText("Builder agent")).toHaveValue("agy");
    expect(within(form).getByLabelText("Builder model")).toHaveValue("g2");
    expect(within(form).getByLabelText("Test Writer agent")).toHaveValue("");
    fireEvent.change(within(form).getByLabelText("Workflow"), { target: { value: "default" } });
    expect(within(form).getByLabelText("Reviewer agent")).toHaveValue("");
  });

  it("starts a workflow with a role keyed like a member every object has", async () => {
    const roles = [{ role: "constructor", label: "Builder", agent: "claude", model: null }];
    const start: FakeStart = { options: { ...OPTIONS, workflows: [{ name: "default", description: "", roles }] } };
    const { form } = await openForm(start);
    typeTask(form, "Add a rate limiter");
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    await waitFor(() => expect(start.requests).toHaveLength(1));
    expect(start.requests?.[0]?.body).toEqual({ task: "Add a rate limiter", workflow: "default", no_pr: false, worktree: true });
  });

  it("sends the roles' agents and models that are set, and no others", async () => {
    const start: FakeStart = {};
    const { form } = await openForm(start);
    typeTask(form, "Add a rate limiter");
    fireEvent.change(within(form).getByLabelText("Reviewer agent"), { target: { value: "codex" } });
    fireEvent.change(within(form).getByLabelText("Builder model"), { target: { value: " g2 " } });
    fireEvent.change(within(form).getByLabelText("Reviewer model"), { target: { value: "  " } });
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    await waitFor(() => expect(start.requests).toHaveLength(1));
    expect(start.requests?.[0]?.body).toEqual({
      task: "Add a rate limiter",
      workflow: "default",
      role_agents: { review: "codex" },
      role_models: { build: "g2" },
      no_pr: false,
      worktree: true,
    });
  });
});
