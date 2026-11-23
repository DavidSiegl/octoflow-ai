import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { AGENT_POLL_MS } from "./components/AgentPanes";
import { INSTALL_POLL_MS } from "./components/SetupDialog";
import type { AdapterInstall, ForgottenRun } from "./api";
import { CONNECTION, FakeWindow, FakeEventSource, OPTIONS, RECORDED, stubServer, type FakeClose, type FakeForget, type FakeResume, type FakeSetup, type FakeStart } from "./test/fakes";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const RUN = "20261024-140000-c0ffee";
const DONE = "20261024-090000-beef00";
const connect = () => Promise.resolve(CONNECTION);

const SPEC = "# Add a token-bucket rate limiter\n\n- **10** requests a second\n\n| Limit | Value |\n|---|---|\n| burst | 20 |\n\n<script>alert('x')</script>\n<b>raw</b>\n";

async function openApp(files: Record<string, string> = {}, agents: Record<string, string> = {}) {
  const fetch = stubServer(files, agents);
  render(<App connect={connect} appWindow={new FakeWindow()} />);
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
    expect(within(done).getByRole("button", { name: "#1" })).toHaveAttribute("title", "https://github.com/parity/project/pull/1");
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
    render(<App connect={connect} appWindow={new FakeWindow()} />);
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

  it("leaves closed runs out until they are asked for", async () => {
    const { source } = await openApp();
    act(() => source.replay("the stream opened"));
    expect(screen.queryByRole("checkbox", { name: /closed runs/ })).not.toBeInTheDocument();
    act(() => source.replayOne("a run started its interview"));
    const batch = RECORDED.find((b) => b.after === "the stream opened");
    const done = batch?.events.find((e) => e.kind === "record" && e.run === DONE)?.record as Record<string, unknown>;

    // Closed as a clean-up's records arrive: the row goes, and the checkbox counts it.
    act(() => source.emit("record", { run: DONE, record: { ...done, closed: "workspace closed" } }));
    expect(document.querySelector(`tr[data-run="${DONE}"]`)).toBeNull();
    expect(row(RUN)).toBeInTheDocument();
    const show = screen.getByRole("checkbox", { name: "Show closed runs (1)" });
    expect(show).not.toBeChecked();

    fireEvent.click(show);
    expect(within(row(DONE)).getByText("APPROVE; closed: workspace closed")).toBeInTheDocument();
    // A closed run's detail, and back: the closed runs are still shown.
    fireEvent.click(row(DONE));
    fireEvent.click(screen.getByRole("button", { name: "‹ Runs" }));
    expect(screen.getByRole("checkbox", { name: "Show closed runs (1)" })).toBeChecked();
    expect(row(DONE)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: "Show closed runs (1)" }));
    expect(document.querySelector(`tr[data-run="${DONE}"]`)).toBeNull();

    // Every run closed: the overview says so rather than that there are none.
    act(() => source.replay("the finished run was deleted"));
    act(() => source.emit("record", { run: DONE, record: { ...done, closed: "workspace closed" } }));
    const running = RECORDED.find((b) => b.after === "a run started its interview")?.events.find((e) => e.kind === "record")?.record as Record<string, unknown>;
    act(() => source.emit("record", { run: RUN, record: { ...running, closed: "workspace closed" } }));
    expect(screen.getByText("No open runs.")).toBeInTheDocument();
    expect(screen.queryByText("No runs.")).not.toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Show closed runs (2)" })).toBeInTheDocument();
  });

  it("opens another project and remembers it", async () => {
    const { source } = await openApp();
    fireEvent.change(screen.getByLabelText("Project folder"), { target: { value: " ~/other " } });
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(source.closed).toBe(true);
    const url = new URL(FakeEventSource.last.url);
    expect(url.searchParams.get("cwd")).toBe("~/other");
    expect(JSON.parse(localStorage.getItem("orchestrator.project") ?? "")).toEqual({ cwd: "~/other" });
  });

  it("opens a project saved with a machine here, saying once that machines are the command line's", async () => {
    localStorage.setItem("orchestrator.project", JSON.stringify({ cwd: "~/remote", machine: "devbox" }));
    await openApp();
    const url = new URL(FakeEventSource.last.url);
    expect(url.searchParams.get("cwd")).toBe("~/remote");
    expect(url.searchParams.has("machine")).toBe(false);
    const note = screen.getByText(/^This project was opened on the machine devbox/);
    expect(note).toHaveTextContent("octoflow run --machine devbox");
    expect(JSON.parse(localStorage.getItem("orchestrator.project") ?? "")).toEqual({ cwd: "~/remote" });
    fireEvent.click(within(note).getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText(/^This project was opened on the machine/)).not.toBeInTheDocument();
  });

  it("drops the note of a project saved with a machine once another project opens", async () => {
    localStorage.setItem("orchestrator.project", JSON.stringify({ cwd: "~/remote", machine: "devbox" }));
    await openApp();
    expect(screen.getByText(/^This project was opened on the machine devbox/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Project folder"), { target: { value: "~/other" } });
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(screen.queryByText(/^This project was opened on the machine/)).not.toBeInTheDocument();
  });

  it("asks for a project when the orchestrator belongs to no checkout", async () => {
    stubServer({});
    render(<App connect={() => Promise.resolve({ ...CONNECTION, project: null })} appWindow={new FakeWindow()} />);
    expect(await screen.findByText("Open a project folder to see its runs.")).toBeInTheDocument();
    expect(FakeEventSource.opened).toHaveLength(0);
    fireEvent.change(screen.getByLabelText("Project folder"), { target: { value: "~/proj" } });
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(new URL(FakeEventSource.last.url).searchParams.get("cwd")).toBe("~/proj");
  });

  it("says why the orchestrator could not start", async () => {
    render(<App connect={() => Promise.reject(new Error("sidecar: not found"))} appWindow={new FakeWindow()} />);
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
    // The old text stays on show until the new arrives, and has a "20" of its own in its table.
    expect(await within(detail).findByText("20", { selector: "strong" })).toBeInTheDocument();
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

  it("opens a run's pull request outside the app, from the overview without selecting the run, and from its detail", async () => {
    invoke.mockReset().mockResolvedValue(undefined);
    const { source } = await openApp({ "review-1.md": "VERDICT: APPROVE\n" });
    act(() => source.replay("the stream opened"));
    const target = { page: "https://github.com/parity/project/pull/1" };
    fireEvent.click(within(row(DONE)).getByRole("button", { name: "#1" }));
    expect(invoke).toHaveBeenLastCalledWith("open_outside", { target });
    expect(screen.queryByRole("region", { name: "run beef00" })).not.toBeInTheDocument();
    invoke.mockRejectedValueOnce("no browser");
    await act(async () => fireEvent.click(within(row(DONE)).getByRole("button", { name: "#1" })));
    fireEvent.click(within(row(DONE)).getByText("Could not open it: no browser"));
    expect(screen.queryByRole("region", { name: "run beef00" })).not.toBeInTheDocument();
    fireEvent.click(row(DONE));
    const detail = screen.getByRole("region", { name: "run beef00" });
    fireEvent.click(within(detail).getByRole("button", { name: target.page }));
    expect(invoke).toHaveBeenCalledTimes(3);
    expect(invoke).toHaveBeenLastCalledWith("open_outside", { target });
  });
});

describe("resuming a run", () => {
  /** The app on a server that answers POST /resume as resume says, its stream showing the finished run DONE as a
   * desktop app's run with resumable and the detail of DONE open. */
  async function openResumable(resume: FakeResume, resumable = true) {
    stubServer({}, {}, {}, {}, {}, {}, {}, resume);
    render(<App connect={connect} appWindow={new FakeWindow()} />);
    await waitFor(() => expect(FakeEventSource.opened).toHaveLength(1));
    const source = FakeEventSource.last;
    const done = RECORDED[0]?.events.find((e) => e.kind === "record")?.record as Record<string, unknown>;
    act(() => source.emit("record", { run: DONE, record: { ...done, backend: "acp", resumable, error: "interrupted" } }));
    fireEvent.click(row(DONE));
    return source;
  }

  it("resumes a resumable run of the app's once while the resume is under way", async () => {
    let release = () => {};
    const resume: FakeResume = { held: new Promise((resolve) => (release = resolve)) };
    await openResumable(resume);
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    const busy = screen.getByRole("button", { name: "Resuming…" });
    expect(busy).toBeDisabled();
    fireEvent.click(busy);
    await act(async () => release());
    expect(await screen.findByRole("button", { name: "Resume" })).toBeEnabled();
    expect(resume.requests).toEqual([{ cwd: CONNECTION.project, run: DONE }]);
  });

  it("says why a resume failed", async () => {
    await openResumable({ status: 422, body: { error: "the resume of run x exited before it took the run up", run: DONE } });
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(await screen.findByText("the resume of run x exited before it took the run up")).toBeInTheDocument();
  });

  it("offers no resume for a run that is not resumable, or not the app's", async () => {
    const source = await openResumable({}, false);
    expect(screen.queryByRole("button", { name: "Resume" })).not.toBeInTheDocument();
    const done = RECORDED[0]?.events.find((e) => e.kind === "record")?.record as Record<string, unknown>;
    act(() => source.emit("record", { run: DONE, record: { ...done, backend: "herdr", resumable: true } }));
    expect(screen.queryByRole("button", { name: "Resume" })).not.toBeInTheDocument();
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
    render(<App connect={connect} appWindow={new FakeWindow()} />);
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
    render(<App connect={connect} appWindow={new FakeWindow()} />);
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
    adapters: [],
  } as const;
  const FAILED = { checks: [{ name: "git", level: "required", ok: false, detail: "git is not on PATH", fix: "install git 2.36 or later" }], passed: false, adapters: [] } as const;
  const CLAUDE_MISSING = {
    checks: [{ name: "claude", level: "required", ok: false, detail: "claude's ACP adapter is not installed", fix: "install it from the setup dialog" }],
    passed: false,
    adapters: [
      { agent: "claude", installed: false, megabytes: 913, installing: false, error: null, warning: null },
      { agent: "codex", installed: true, megabytes: 649, installing: false, error: null, warning: null },
    ],
  } as const;
  const CLAUDE_INSTALLED = {
    checks: [{ name: "claude", level: "required", ok: true, detail: "claude-agent-acp 0.101.0", fix: "" }],
    passed: true,
    adapters: [
      { agent: "claude", installed: true, megabytes: 913, installing: false, error: null, warning: null },
      { agent: "codex", installed: true, megabytes: 649, installing: false, error: null, warning: null },
    ],
  } as const;
  const CODEX_MISSING = {
    checks: [
      { name: "claude", level: "required", ok: true, detail: "claude-agent-acp 0.101.0", fix: "" },
      { name: "codex", level: "optional", ok: false, detail: "codex's ACP adapter is not installed", fix: "install it" },
    ],
    passed: true,
    adapters: [
      { agent: "claude", installed: true, megabytes: 913, installing: false, error: null, warning: null },
      { agent: "codex", installed: false, megabytes: 649, installing: false, error: null, warning: null },
    ],
  } as const;
  const INSTALL = "Install claude's ACP adapter (about 913 MB)";

  async function openWith(setup: FakeSetup) {
    const fetch = stubServer({}, {}, setup);
    render(<App connect={connect} appWindow={new FakeWindow()} />);
    await waitFor(() => expect(FakeEventSource.opened).toHaveLength(1));
    return fetch;
  }

  function setupCalls(fetch: ReturnType<typeof stubServer>) {
    return fetch.mock.calls.map(([url]) => new URL(url as string)).filter((url) => url.pathname === "/setup");
  }

  it("opens by itself without the marker, and checks the open project", async () => {
    const fetch = await openWith({ checked: false, report: { ...REPORT, checks: [...REPORT.checks], adapters: [...REPORT.adapters] } });
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
    const fetch = await openWith({ checked: true, report: { ...REPORT, checks: [...REPORT.checks], adapters: [...REPORT.adapters] } });
    fireEvent.change(screen.getByLabelText("Project folder"), { target: { value: "~/elsewhere" } });
    fireEvent.click(screen.getByRole("button", { name: "Check setup…" }));
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    expect(within(dialog).getByText("/home/me/proj on this computer")).toBeInTheDocument();
    await within(dialog).findByText("herdr 0.9.1", { exact: false });
    expect(setupCalls(fetch)[0]?.searchParams.get("cwd")).toBe("/home/me/proj");
  });

  it("writes the marker on Close when every required check passed", async () => {
    const setup: FakeSetup = { checked: false, report: { ...REPORT, checks: [...REPORT.checks], adapters: [...REPORT.adapters] } };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("herdr 0.9.1", { exact: false });
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(setup.marked).toBe(1);
  });

  it("writes the marker once for a double click on Close", async () => {
    const setup: FakeSetup = { checked: false, report: { ...REPORT, checks: [...REPORT.checks], adapters: [...REPORT.adapters] } };
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
    const setup: FakeSetup = { checked: false, report: { ...FAILED, checks: [...FAILED.checks], adapters: [...FAILED.adapters] } };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    expect(await within(dialog).findByText("fix: install git 2.36 or later")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(setup.marked).toBeUndefined();
  });

  it("shows a marker that cannot be written, and closes without it the next time", async () => {
    const setup: FakeSetup = { checked: false, report: { ...REPORT, checks: [...REPORT.checks], adapters: [...REPORT.adapters] }, markError: "cannot write /cfg/setup-checked: Permission denied" };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("herdr 0.9.1", { exact: false });
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(await within(dialog).findByText(/Could not record that setup was checked: cannot write \/cfg\/setup-checked/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  // serve's set_up pins no release, so its answer names gate.toml alone, written or kept.
  it.each([
    [true, "wrote"],
    [false, "kept"],
  ])("sets the quality gate up for the open project, and shows gate.toml as written: %s", async (written, word) => {
    const files = [{ path: "gate.toml", written }];
    const setup: FakeSetup = { checked: false, report: { ...REPORT, checks: [...REPORT.checks], adapters: [...REPORT.adapters] }, gateFiles: files };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("herdr 0.9.1", { exact: false });
    expect(within(dialog).queryByRole("region", { name: "Quality gate" })).not.toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Set up the quality gate" }));
    const gate = await within(dialog).findByRole("region", { name: "Quality gate" });
    expect(await within(gate).findByText(word)).toBeInTheDocument();
    expect(within(gate).getByText("gate.toml")).toBeInTheDocument();
    expect(within(gate).queryByText("ci/gate.pin")).not.toBeInTheDocument();
    expect(within(gate).getByText(/reads these from a run's base commit/)).toBeInTheDocument();
    expect(setup.gateCwds).toEqual(["/home/me/proj"]);
  });

  it("shows why the quality gate could not be set up", async () => {
    const setup: FakeSetup = { checked: false, report: { ...REPORT, checks: [...REPORT.checks], adapters: [...REPORT.adapters] }, gateError: "/home/me/proj is in no git checkout" };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("herdr 0.9.1", { exact: false });
    fireEvent.click(within(dialog).getByRole("button", { name: "Set up the quality gate" }));
    const gate = await within(dialog).findByRole("region", { name: "Quality gate" });
    expect(await within(gate).findByText("/home/me/proj is in no git checkout")).toBeInTheDocument();
  });

  it("offers no install of an ACP adapter for app runs in herdr", async () => {
    await openWith({ checked: false, report: { ...REPORT, checks: [...REPORT.checks], adapters: [...REPORT.adapters] } });
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("herdr 0.9.1", { exact: false });
    expect(within(dialog).queryByRole("button", { name: /Install/ })).not.toBeInTheDocument();
  });

  /** A copy of report, which the dialog's fake server may then change, with claude's adapter as fields say. */
  function withClaude(report: typeof CLAUDE_MISSING | typeof CLAUDE_INSTALLED, fields: Partial<AdapterInstall> = {}) {
    const adapters = report.adapters.map((a) => (a.agent === "claude" ? { ...a, ...fields } : { ...a }));
    return { ...report, checks: [...report.checks], adapters };
  }
  const POLLED = { timeout: 5 * INSTALL_POLL_MS + 1000 };

  it("installs codex's ACP adapter by its own button, offering none for claude's, which is installed", async () => {
    const setup: FakeSetup = { checked: false, report: { ...CODEX_MISSING, checks: [...CODEX_MISSING.checks], adapters: [...CODEX_MISSING.adapters] } };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    const button = await within(dialog).findByRole("button", { name: "Install codex's ACP adapter (about 649 MB)" });
    expect(within(dialog).queryByRole("button", { name: /Install claude/ })).not.toBeInTheDocument();
    setup.report = withClaude(CLAUDE_INSTALLED);
    fireEvent.click(button);
    await waitFor(() => expect(within(dialog).queryByRole("button", { name: /Install codex/ })).not.toBeInTheDocument(), POLLED);
    expect(setup.installed).toEqual(["codex"]);
  });

  it("installs claude's ACP adapter, and checks again once its install ends", async () => {
    const setup: FakeSetup = { checked: false, report: withClaude(CLAUDE_MISSING) };
    const fetch = await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("fix: install it from the setup dialog");
    setup.report = withClaude(CLAUDE_INSTALLED);
    fireEvent.click(within(dialog).getByRole("button", { name: INSTALL }));
    expect(await within(dialog).findByText("claude-agent-acp 0.101.0", { exact: false }, POLLED)).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: /Install claude/ })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("region", { name: "claude's ACP adapter" })).not.toBeInTheDocument();
    expect(setup.installed).toEqual(["claude"]);
    expect(setupCalls(fetch)).toHaveLength(2);
  });

  it("shows that claude's ACP adapter is installing, with its button disabled, until the install ends", async () => {
    const setup: FakeSetup = { checked: false, report: withClaude(CLAUDE_MISSING), adapters: withClaude(CLAUDE_MISSING, { installing: true }).adapters };
    const fetch = await openWith(setup);
    const looks = () => fetch.mock.calls.filter(([url]) => new URL(url as string).pathname === "/setup/adapters").length;
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    const button = await within(dialog).findByRole("button", { name: INSTALL });
    fireEvent.click(button);
    const section = within(dialog).getByRole("region", { name: "claude's ACP adapter" });
    expect(within(section).getByText("Installing…")).toBeInTheDocument();
    expect(button).toBeDisabled();
    // Still running at the server's next answer.
    await waitFor(() => expect(looks()).toBeGreaterThan(0), POLLED);
    expect(within(section).getByText("Installing…")).toBeInTheDocument();
    setup.report = withClaude(CLAUDE_INSTALLED);
    setup.adapters = undefined;
    await waitFor(() => expect(within(dialog).queryByText("Installing…")).not.toBeInTheDocument(), POLLED);
    expect(setup.installed).toEqual(["claude"]);
  });

  it("shows an install that runs when the dialog opens, and checks again once it ends", async () => {
    const setup: FakeSetup = { checked: false, report: withClaude(CLAUDE_MISSING, { installing: true }) };
    const fetch = await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    const section = await within(dialog).findByRole("region", { name: "claude's ACP adapter" });
    expect(within(section).getByText("Installing…")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: INSTALL })).toBeDisabled();
    setup.report = withClaude(CLAUDE_INSTALLED);
    expect(await within(dialog).findByText("claude-agent-acp 0.101.0", { exact: false }, POLLED)).toBeInTheDocument();
    expect(within(dialog).queryByText("Installing…")).not.toBeInTheDocument();
    expect(setup.installed).toBeUndefined();
    expect(setupCalls(fetch)).toHaveLength(2);
  });

  it("shows why claude's ACP adapter could not be installed, and offers it again", async () => {
    const setup: FakeSetup = { checked: false, report: withClaude(CLAUDE_MISSING) };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    setup.adapters = withClaude(CLAUDE_MISSING, { error: "node-v22.23.4-linux-x64.tar.gz: sha256 does not match SHASUMS256.txt" }).adapters;
    fireEvent.click(await within(dialog).findByRole("button", { name: INSTALL }));
    const section = await within(dialog).findByRole("region", { name: "claude's ACP adapter" });
    expect(await within(section).findByText(/sha256 does not match/, {}, POLLED)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: INSTALL }));
    await waitFor(() => expect(setup.installed).toEqual(["claude", "claude"]));
  });

  it("shows why an install could not start", async () => {
    const setup: FakeSetup = { checked: false, report: withClaude(CLAUDE_MISSING), startError: "cannot start the install of claude's ACP adapter: no threads" };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    fireEvent.click(await within(dialog).findByRole("button", { name: INSTALL }));
    expect(await within(dialog).findByText(/no threads/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: INSTALL })).toBeEnabled();
  });

  it("shows the old install an install left beside claude's installed adapter", async () => {
    const left = "cannot remove /data/claude-acp/node-22.1.0-adapter-0.9.0: Permission denied";
    const setup: FakeSetup = { checked: false, report: withClaude(CLAUDE_MISSING) };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    setup.report = withClaude(CLAUDE_INSTALLED, { warning: left });
    fireEvent.click(await within(dialog).findByRole("button", { name: INSTALL }));
    expect(await within(dialog).findByText("claude-agent-acp 0.101.0", { exact: false }, POLLED)).toBeInTheDocument();
    const section = within(dialog).getByRole("region", { name: "claude's ACP adapter" });
    expect(within(section).getByText(left, { exact: false })).toBeInTheDocument();
  });

  it("stops showing a failed install once a round of checks finds claude's ACP adapter installed", async () => {
    const setup: FakeSetup = { checked: false, report: withClaude(CLAUDE_MISSING, { error: "cannot download node: HTTP 503" }) };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    expect(await within(dialog).findByText("cannot download node: HTTP 503")).toBeInTheDocument();
    // Installed by other means, as by another octoflow, the error it reports still.
    setup.report = withClaude(CLAUDE_INSTALLED, { error: "cannot download node: HTTP 503" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Check again" }));
    expect(await within(dialog).findByText("claude-agent-acp 0.101.0", { exact: false })).toBeInTheDocument();
    expect(within(dialog).queryByText("cannot download node: HTTP 503")).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: /Install claude/ })).not.toBeInTheDocument();
    // Nor while the next round runs, nor after it.
    fireEvent.click(within(dialog).getByRole("button", { name: "Check again" }));
    expect(within(dialog).queryByText("cannot download node: HTTP 503")).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: /Install claude/ })).not.toBeInTheDocument();
    await within(dialog).findByText("claude-agent-acp 0.101.0", { exact: false });
    expect(within(dialog).queryByText("cannot download node: HTTP 503")).not.toBeInTheDocument();
  });

  it("keeps following an install that a round's older answer does not know of", async () => {
    const setup: FakeSetup = { checked: false, report: withClaude(CLAUDE_MISSING) };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    const button = await within(dialog).findByRole("button", { name: INSTALL });
    let release = () => {};
    setup.reportHeld = new Promise((resolve) => (release = resolve));
    fireEvent.click(within(dialog).getByRole("button", { name: "Check again" }));
    // The round's report is taken before the install starts, and answers after the install's 202.
    await waitFor(() => expect(within(dialog).getByText("Checking…")).toBeInTheDocument());
    setup.adapters = withClaude(CLAUDE_MISSING, { installing: true }).adapters;
    fireEvent.click(button);
    await waitFor(() => expect(setup.installed).toEqual(["claude"]));
    await within(within(dialog).getByRole("region", { name: "claude's ACP adapter" })).findByText("Installing…");
    setup.reportHeld = undefined;
    release();
    await within(dialog).findByText("fix: install it from the setup dialog");
    expect(within(dialog).getByText("Installing…")).toBeInTheDocument();
    // Still followed: its end runs the checks again.
    setup.report = withClaude(CLAUDE_INSTALLED);
    setup.adapters = undefined;
    expect(await within(dialog).findByText("claude-agent-acp 0.101.0", { exact: false }, POLLED)).toBeInTheDocument();
    expect(within(dialog).queryByText("Installing…")).not.toBeInTheDocument();
  });

  it("stops following an install the server cannot tell of, saying why", async () => {
    const setup: FakeSetup = { checked: false, report: withClaude(CLAUDE_MISSING, { installing: true }), adaptersError: "HTTP 500" };
    await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    expect(await within(dialog).findByText("cannot tell how the install goes: HTTP 500", {}, { timeout: 10 * INSTALL_POLL_MS + 2000 })).toBeInTheDocument();
    expect(within(dialog).queryByText("Installing…")).not.toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: INSTALL })).toBeEnabled();
  });

  it("checks again", async () => {
    const setup: FakeSetup = { checked: false, report: { ...FAILED, checks: [...FAILED.checks], adapters: [...FAILED.adapters] } };
    const fetch = await openWith(setup);
    const dialog = await screen.findByRole("dialog", { name: "setup" });
    await within(dialog).findByText("fix: install git 2.36 or later");
    setup.report = { ...REPORT, checks: [...REPORT.checks], adapters: [...REPORT.adapters] };
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
    render(<App connect={connect} appWindow={new FakeWindow()} />);
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
    expect(within(form).getByLabelText("Permission mode")).toHaveValue("default");
    typeTask(form, " Add a rate limiter ");
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(start.requests).toEqual([
      { cwd: "/home/me/proj", body: { task: "Add a rate limiter", workflow: "default", permission_mode: "default", no_pr: false, worktree: true } },
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
      permission_mode: "default",
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

  // jsdom keeps an inert element focusable and clickable, so these check for the attribute the webview enforces.
  const reachable = (element: HTMLElement) => element.closest("[inert]") === null;

  it("leaves only the form reachable while a start is pending, and then selects the started run", async () => {
    let release = () => {};
    const { form, source } = await openForm({ held: new Promise<void>((resolve) => (release = resolve)) });
    expect(reachable(form)).toBe(true);
    typeTask(form, "Add a rate limiter");
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    await within(form).findByText("Starting the run…");
    expect(reachable(screen.getByLabelText("Project folder"))).toBe(false);
    expect(reachable(screen.getByRole("button", { name: "Check setup…" }))).toBe(false);
    await act(async () => release());
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(reachable(screen.getByLabelText("Project folder"))).toBe(true);
    act(() => source.replay("a run started its interview"));
    expect(screen.getByRole("region", { name: "run c0ffee" })).toBeInTheDocument();
  });

  it("leaves only the form reachable while a start is pending, and then shows why it failed", async () => {
    let release = () => {};
    const held = new Promise<void>((resolve) => (release = resolve));
    const { form } = await openForm({ held, runStatus: 422, runBody: { error: "no workflow nope", run: RUN } });
    typeTask(form, "Add a rate limiter");
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    await within(form).findByText("Starting the run…");
    expect(reachable(screen.getByRole("button", { name: "Open" }))).toBe(false);
    await act(async () => release());
    expect(await within(form).findByText("no workflow nope")).toBeInTheDocument();
    expect(reachable(screen.getByLabelText("Project folder"))).toBe(false);
  });

  it("says a run is not listed yet, and opens it once it is", async () => {
    const { form, source } = await openForm({ runStatus: 202, runBody: { run: RUN, listed: false } });
    typeTask(form, "Add a rate limiter");
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    const hint = await screen.findByText(new RegExp(`^Run ${RUN} has started but is not listed yet`));
    expect(hint).toHaveTextContent(`If it never is, its log, .octoflow-ai/runs/${RUN}/run.log in the project, shows why.`);
    act(() => source.replay("a run started its interview"));
    expect(screen.getByRole("region", { name: "run c0ffee" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "‹ Runs" }));
    expect(screen.queryByText(/is not listed yet/)).not.toBeInTheDocument();
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
    expect(start.requests?.[0]?.body.permission_mode).toBe("default");
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

  it("offers each permission mode, says what plan does, and sends the one chosen", async () => {
    const start: FakeStart = {};
    const { form } = await openForm(start);
    const mode = within(form).getByLabelText("Permission mode");
    expect(within(mode).getAllByRole("option").map((o) => o.textContent)).toEqual([
      "default",
      "acceptEdits",
      "plan: agents write nothing, so the run stops at its first handoff",
      "bypassPermissions",
    ]);
    typeTask(form, "Add a rate limiter");
    fireEvent.change(mode, { target: { value: "acceptEdits" } });
    fireEvent.click(within(form).getByRole("button", { name: "Start" }));
    await waitFor(() => expect(start.requests).toHaveLength(1));
    expect(start.requests?.[0]?.body.permission_mode).toBe("acceptEdits");
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
    expect(start.requests?.[0]?.body).toEqual({ task: "Add a rate limiter", workflow: "default", permission_mode: "default", no_pr: false, worktree: true });
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
      permission_mode: "default",
      no_pr: false,
      worktree: true,
    });
  });
});

describe("cleaning up runs", () => {
  const LOST = "20261024-100000-dead00";
  const OLD = "20261023-080000-0ld000";
  const PREVIEW = [
    { run: DONE, outcome: "closed", detail: "close workspace w1", discarded: [] },
    { run: LOST, outcome: "closed", detail: "delete worktree /wt/dead00; delete branch orchestrator/x-dead00", discarded: [" M app.py", "?? new.py"] },
    { run: RUN, outcome: "skipped", detail: `run ${RUN} is still running (beat 3s ago); stop it before closing it`, discarded: [] },
    { run: OLD, outcome: "already closed", detail: "workspace closed", discarded: [] },
  ] as const;

  /** The app, its stream having listed DONE, with the dialog open once its dry run is answered. */
  async function openCleanUp(close: FakeClose) {
    stubServer({}, {}, {}, {}, close);
    render(<App connect={connect} appWindow={new FakeWindow()} />);
    await waitFor(() => expect(FakeEventSource.opened).toHaveLength(1));
    act(() => FakeEventSource.last.replay("the stream opened"));
    fireEvent.click(screen.getByRole("button", { name: "Clean up runs…" }));
    const dialog = screen.getByRole("dialog", { name: "clean up runs" });
    await waitFor(() => expect(within(dialog).queryByText("Looking at the runs…")).not.toBeInTheDocument());
    return dialog;
  }

  /** The dialog's line for run. */
  function line(dialog: HTMLElement, run: string): HTMLElement {
    const element = dialog.querySelector<HTMLElement>(`li[data-run="${run}"]`);
    if (!element) {
      throw new Error(`no line for ${run}`);
    }
    return element;
  }

  it("lists what closing each run would do, by its task, and the changes that go with a worktree", async () => {
    const close: FakeClose = { preview: [...PREVIEW].map((r) => ({ ...r, discarded: [...r.discarded] })) };
    const dialog = await openCleanUp(close);
    expect(line(dialog, DONE).textContent).toBe("close add a token-bucket rate limiter to the API client: close workspace w1");
    expect(line(dialog, LOST)).toHaveAttribute("data-state", "ok");
    expect(within(line(dialog, LOST)).getByText(LOST)).toBeInTheDocument();
    expect(within(line(dialog, LOST)).getByText("Uncommitted changes that go with the worktree:")).toBeInTheDocument();
    expect(line(dialog, LOST).querySelector("pre")?.textContent).toBe(" M app.py\n?? new.py");
    expect(line(dialog, RUN).textContent).toContain("skip");
    expect(line(dialog, RUN).textContent).toContain("is still running");
    expect(dialog.querySelector(`li[data-run="${OLD}"]`)).toBeNull();
    expect(within(dialog).getByText("1 run was closed before.")).toBeInTheDocument();
    expect(close.requests).toEqual([{ cwd: "/home/me/proj", body: { dry_run: true } }]);
  });

  it("closes the runs on Close, and shows each one's result, a failure with its run", async () => {
    let release = () => {};
    const held = new Promise<void>((resolve) => (release = resolve));
    const closed = [
      { run: DONE, outcome: "closed", detail: "workspace closed", discarded: [] },
      { run: LOST, outcome: "failed", detail: "git worktree remove failed: permission denied", discarded: [] },
      { run: RUN, outcome: "skipped", detail: "still running", discarded: [] },
    ] as FakeClose["closed"];
    const close: FakeClose = { preview: [...PREVIEW].map((r) => ({ ...r, discarded: [...r.discarded] })), closed, held };
    const dialog = await openCleanUp(close);
    fireEvent.click(within(dialog).getByRole("button", { name: "Close 2 runs" }));
    expect(within(dialog).getByText("Closing…")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Done" })).toBeDisabled();
    expect(within(dialog).queryByRole("button", { name: /^Close/ })).not.toBeInTheDocument();
    release();
    await waitFor(() => expect(within(dialog).queryByText("Closing…")).not.toBeInTheDocument());
    // Close sends the runs the preview would close, as it showed them, so that the server closes no other.
    expect(close.requests?.map((r) => r.body)).toEqual([
      { dry_run: true },
      { close: [PREVIEW[0], PREVIEW[1]].map(({ run, detail, discarded }) => ({ run, detail, discarded: [...discarded] })) },
    ]);
    expect(line(dialog, DONE).textContent).toContain("closed");
    expect(line(dialog, LOST)).toHaveAttribute("data-state", "missing");
    expect(line(dialog, LOST).textContent).toBe(`failed ${LOST}: git worktree remove failed: permission denied`);
    fireEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    // The overview follows the close through its stream, which sends the run's state.json as closing recorded it:
    // the closed run leaves the overview, and shows again among the closed ones.
    const done = RECORDED[0]?.events.find((e) => e.kind === "record")?.record as Record<string, unknown>;
    act(() => FakeEventSource.last.emit("record", { run: DONE, record: { ...done, closed: "workspace closed" } }));
    expect(document.querySelector(`tr[data-run="${DONE}"]`)).toBeNull();
    fireEvent.click(screen.getByRole("checkbox", { name: /^Show closed runs/ }));
    expect(row(DONE).textContent).toContain("closed: workspace closed");
  });

  it("says there is nothing to close, and offers no Close", async () => {
    const close: FakeClose = { preview: [{ ...PREVIEW[2], discarded: [] }, { ...PREVIEW[3], discarded: [] }] };
    const dialog = await openCleanUp(close);
    expect(within(dialog).getByText("Nothing to close.")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: /^Close/ })).not.toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(close.requests).toHaveLength(1);
  });

  it("says which runs could not be looked at, and offers Close only for the others", async () => {
    const unread = { run: LOST, outcome: "failed", detail: "herdr server_error: herdr is down", discarded: [] } as const;
    const alone = await openCleanUp({ preview: [{ ...unread, discarded: [] }] });
    expect(within(alone).getByText("1 run could not be looked at, so it stays open.")).toHaveClass("problem");
    expect(within(alone).queryByText("Nothing to close.")).not.toBeInTheDocument();
    expect(within(alone).queryByRole("button", { name: /^Close/ })).not.toBeInTheDocument();
    cleanup();

    const beside = await openCleanUp({ preview: [{ ...PREVIEW[0], discarded: [] }, { ...unread, discarded: [] }] });
    expect(within(beside).getByText("1 run could not be looked at, so it stays open.")).toBeInTheDocument();
    expect(within(beside).getByRole("button", { name: "Close 1 run" })).toBeInTheDocument();
  });

  it("shows why the runs could not be looked at", async () => {
    const dialog = await openCleanUp({ error: "ssh: connection refused" });
    expect(within(dialog).getByText("ssh: connection refused")).toHaveClass("problem");
    expect(within(dialog).queryByText("Nothing to close.")).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: /^Close/ })).not.toBeInTheDocument();
  });
});

describe("deleting closed runs", () => {
  const OLD = "20261023-080000-0ld000";
  const WAITING = "20261022-070000-5a1700";
  const PREVIEW: ForgottenRun[] = [
    { run: DONE, outcome: "forgotten", detail: "" },
    { run: OLD, outcome: "forgotten", detail: "" },
    { run: WAITING, outcome: "waiting for prune", detail: "prune deletes its branch and worktree once its pull request is merged or closed" },
    { run: RUN, outcome: "not closed", detail: "" },
  ];

  /** The app, its stream having listed DONE and OLD, both closed, OLD with a task of its own. */
  async function openClosedRuns(forget: FakeForget) {
    stubServer({}, {}, {}, {}, {}, {}, forget);
    render(<App connect={connect} appWindow={new FakeWindow()} />);
    await waitFor(() => expect(FakeEventSource.opened).toHaveLength(1));
    const source = FakeEventSource.last;
    act(() => source.replay("the stream opened"));
    expect(screen.queryByRole("button", { name: "Delete closed runs…" })).not.toBeInTheDocument();
    const done = RECORDED[0]?.events.find((e) => e.kind === "record")?.record as Record<string, unknown>;
    act(() => source.emit("record", { run: DONE, record: { ...done, closed: "workspace closed" } }));
    act(() => source.emit("record", { run: OLD, record: { ...done, id: OLD, task: "retry failed uploads", closed: "workspace closed" } }));
    return source;
  }

  /** The dialog, opened from the overview once its dry run is answered. */
  async function openDialog() {
    fireEvent.click(screen.getByRole("button", { name: "Delete closed runs…" }));
    const dialog = screen.getByRole("dialog", { name: "delete closed runs" });
    await waitFor(() => expect(within(dialog).queryByText("Looking at the runs…")).not.toBeInTheDocument());
    return dialog;
  }

  /** The dialog's line for run. */
  function line(dialog: HTMLElement, run: string): HTMLElement {
    const element = dialog.querySelector<HTMLElement>(`li[data-run="${run}"]`);
    if (!element) {
      throw new Error(`no line for ${run}`);
    }
    return element;
  }

  it("lists the closed runs it would delete by task, and counts those waiting for prune", async () => {
    const forget: FakeForget = { preview: [...PREVIEW, { run: "20261021-060000-bad000", outcome: "skipped", detail: "state.json: unexpected end of file" }] };
    await openClosedRuns(forget);
    const dialog = await openDialog();
    expect(line(dialog, DONE).textContent).toBe("delete add a token-bucket rate limiter to the API client");
    expect(line(dialog, OLD).textContent).toBe("delete retry failed uploads");
    expect(line(dialog, "20261021-060000-bad000")).toHaveAttribute("data-state", "warn");
    expect(line(dialog, "20261021-060000-bad000").textContent).toBe("skip 20261021-060000-bad000: state.json: unexpected end of file");
    expect(dialog.querySelector(`li[data-run="${RUN}"]`)).toBeNull();
    expect(dialog.querySelector(`li[data-run="${WAITING}"]`)).toBeNull();
    expect(within(dialog).getByText("1 run waits for prune to delete its branch first.")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Delete 2 runs" })).toBeInTheDocument();
    expect(forget.requests).toEqual([{ cwd: "/home/me/proj", body: { dry_run: true } }]);
  });

  it("deletes the runs on Delete, shows how each went, and the overview drops them as its stream does", async () => {
    let release = () => {};
    const held = new Promise<void>((resolve) => (release = resolve));
    const forgotten: ForgottenRun[] = [
      { run: DONE, outcome: "forgotten", detail: "" },
      { run: OLD, outcome: "failed", detail: "remove /proj/.octoflow-ai/runs/x: permission denied" },
    ];
    const forget: FakeForget = { preview: PREVIEW, forgotten, held };
    const source = await openClosedRuns(forget);
    expect(screen.getByRole("checkbox", { name: "Show closed runs (2)" })).toBeInTheDocument();
    const dialog = await openDialog();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete 2 runs" }));
    expect(within(dialog).getByText("Deleting…")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Done" })).toBeDisabled();
    expect(within(dialog).queryByRole("button", { name: /^Delete/ })).not.toBeInTheDocument();

    // The stream reports the deleted run gone before the answer comes; the dialog still names it by its task.
    act(() => source.emit("record", { run: DONE, record: null }));
    release();
    await waitFor(() => expect(within(dialog).queryByText("Deleting…")).not.toBeInTheDocument());
    expect(forget.requests?.map((r) => r.body)).toEqual([{ dry_run: true }, { forget: [DONE, OLD] }]);
    expect(line(dialog, DONE).textContent).toBe("deleted add a token-bucket rate limiter to the API client");
    expect(line(dialog, OLD)).toHaveAttribute("data-state", "missing");
    expect(line(dialog, OLD).textContent).toBe("failed retry failed uploads: remove /proj/.octoflow-ai/runs/x: permission denied");
    fireEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    // The deleted run has left the overview and the checkbox's count; once no closed run is left, so have both.
    expect(screen.getByRole("checkbox", { name: "Show closed runs (1)" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: "Show closed runs (1)" }));
    expect(document.querySelector(`tr[data-run="${DONE}"]`)).toBeNull();
    expect(row(OLD)).toBeInTheDocument();
    act(() => source.emit("record", { run: OLD, record: null }));
    expect(screen.queryByRole("checkbox", { name: /closed runs/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete closed runs…" })).not.toBeInTheDocument();
  });

  it("says there is nothing to delete, and offers no Delete", async () => {
    const forget: FakeForget = { preview: [PREVIEW[2], PREVIEW[3]] as ForgottenRun[] };
    await openClosedRuns(forget);
    const dialog = await openDialog();
    expect(within(dialog).getByText("Nothing to delete.")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: /^Delete/ })).not.toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(forget.requests).toHaveLength(1);
  });

  it("shows why the runs could not be looked at", async () => {
    await openClosedRuns({ error: "ssh: connection refused" });
    const dialog = await openDialog();
    expect(within(dialog).getByText("ssh: connection refused")).toHaveClass("problem");
    expect(within(dialog).queryByText("Nothing to delete.")).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: /^Delete/ })).not.toBeInTheDocument();
  });
});
