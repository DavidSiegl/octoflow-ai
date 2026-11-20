import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { App } from "../App";
import { CLOSE_CHECK_WAIT } from "./CloseGuard";
import { CONNECTION, FakeEventSource, FakeWindow, stubServer, type FakeStop } from "../test/fakes";

const invoke = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const RUN = "20261024-140000-c0ffee";
const OTHER = "20261024-150000-0ff1ce";

/** The app on a server whose /stop and /close-choice answer as stop says, in a window the test closes. */
async function openApp(stop: FakeStop) {
  stubServer({}, {}, {}, {}, {}, {}, {}, {}, stop);
  const win = new FakeWindow();
  render(<App connect={() => Promise.resolve(CONNECTION)} appWindow={win} />);
  await waitFor(() => expect(FakeEventSource.opened).toHaveLength(1));
  return win;
}

/** Ask win to close, and settle what the app does about it. */
async function close(win: FakeWindow) {
  await act(() => win.requestClose());
}

describe("closing the app", () => {
  it("closes at once with no run running", async () => {
    const stop: FakeStop = {};
    const win = await openApp(stop);
    await close(win);
    expect(win.destroyed).toBe(true);
    expect(screen.queryByRole("dialog", { name: "close the app" })).not.toBeInTheDocument();
    expect(stop.stops).toBeUndefined();
  });

  it("asks with runs running, focused on Keep running, which closes without stopping them", async () => {
    const stop: FakeStop = { running: [{ run: RUN, error: null }, { run: OTHER, error: null }] };
    const win = await openApp(stop);
    await close(win);
    expect(win.destroyed).toBe(false);
    const dialog = screen.getByRole("dialog", { name: "close the app" });
    expect(dialog).toHaveTextContent(`2 runs are running: ${RUN}, ${OTHER}.`);
    const keep = within(dialog).getByRole("button", { name: "Keep running" });
    expect(keep).toHaveFocus();
    fireEvent.click(keep);
    await waitFor(() => expect(win.destroyed).toBe(true));
    expect(stop.stops).toBeUndefined();
    expect(stop.choice).toBeUndefined();
  });

  it("stops the runs on Stop all, then closes", async () => {
    let release = () => {};
    const held = new Promise<void>((resolve) => (release = resolve));
    const stop: FakeStop = { running: [{ run: RUN, error: null }], held };
    const win = await openApp(stop);
    await close(win);
    fireEvent.click(screen.getByRole("button", { name: "Stop all" }));
    expect(await screen.findByText("Stopping 1 run…")).toBeInTheDocument();
    expect(win.destroyed).toBe(false);
    release();
    await waitFor(() => expect(win.destroyed).toBe(true));
    expect(stop.stops).toBe(1);
  });

  it("saves the choice made with Don't ask again", async () => {
    const stop: FakeStop = { running: [{ run: RUN, error: null }] };
    const win = await openApp(stop);
    await close(win);
    fireEvent.click(screen.getByRole("checkbox", { name: "Don't ask again" }));
    fireEvent.click(screen.getByRole("button", { name: "Keep running" }));
    await waitFor(() => expect(win.destroyed).toBe(true));
    expect(stop.choice).toBe("keep");
  });

  it("closes without asking or stopping once Keep running was remembered", async () => {
    const stop: FakeStop = { running: [{ run: RUN, error: null }], choice: "keep" };
    const win = await openApp(stop);
    await close(win);
    expect(win.destroyed).toBe(true);
    expect(stop.stops).toBeUndefined();
  });

  it("stops without asking once Stop all was remembered", async () => {
    const stop: FakeStop = { running: [{ run: RUN, error: null }], choice: "stop" };
    const win = await openApp(stop);
    await close(win);
    expect(screen.queryByRole("dialog", { name: "close the app" })).not.toBeInTheDocument();
    await waitFor(() => expect(win.destroyed).toBe(true));
    expect(stop.stops).toBe(1);
  });

  it("closes at once when the server cannot say whether runs are running", async () => {
    const stop: FakeStop = { running: [{ run: RUN, error: null }], choiceError: "permission denied" };
    const win = await openApp(stop);
    await close(win);
    expect(win.destroyed).toBe(true);
  });

  it("closes at once, asking the server nothing about runs, while the app's runs are in herdr", async () => {
    const previewHeld = new Promise<void>(() => {});
    const stop: FakeStop = { running: [{ run: RUN, error: null }], inHerdr: true, previewHeld };
    const win = await openApp(stop);
    await close(win);
    expect(win.destroyed).toBe(true);
  });

  it("closes once the server has not said within CLOSE_CHECK_WAIT whether runs are running", async () => {
    const previewHeld = new Promise<void>(() => {});
    const win = await openApp({ running: [{ run: RUN, error: null }], previewHeld });
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      await act(async () => {
        const closing = win.requestClose();
        await vi.advanceTimersByTimeAsync(CLOSE_CHECK_WAIT);
        await closing;
      });
    } finally {
      vi.useRealTimers();
    }
    expect(win.destroyed).toBe(true);
  });

  it("carries on with a stop when asked to close again meanwhile", async () => {
    let release = () => {};
    const held = new Promise<void>((resolve) => (release = resolve));
    const stop: FakeStop = { running: [{ run: RUN, error: null }], held };
    const win = await openApp(stop);
    await close(win);
    fireEvent.click(screen.getByRole("button", { name: "Stop all" }));
    expect(await screen.findByText("Stopping 1 run…")).toBeInTheDocument();
    await close(win);
    expect(win.destroyed).toBe(false);
    expect(screen.queryByRole("dialog", { name: "close the app" })).not.toBeInTheDocument();
    expect(screen.getByText("Stopping 1 run…")).toBeInTheDocument();
    release();
    await waitFor(() => expect(win.destroyed).toBe(true));
    expect(stop.stops).toBe(1);
  });

  it("shows why the window could not be closed", async () => {
    const win = await openApp({ running: [{ run: RUN, error: null }] });
    win.destroyError = "the window is gone";
    await close(win);
    fireEvent.click(screen.getByRole("button", { name: "Keep running" }));
    const dialog = await screen.findByRole("dialog", { name: "stopping runs" });
    expect(within(dialog).getByText("Could not close the window: the window is gone")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Stay open" })).toBeInTheDocument();
  });

  it("shows a run that did not stop, and stays open or closes anyway as asked", async () => {
    const stop: FakeStop = {
      running: [{ run: RUN, error: null }, { run: OTHER, error: null }],
      stopped: [
        { run: RUN, error: null },
        { run: OTHER, error: "still running a minute after it was told to stop" },
      ],
    };
    const win = await openApp(stop);
    await close(win);
    fireEvent.click(screen.getByRole("button", { name: "Stop all" }));
    const dialog = await screen.findByRole("dialog", { name: "stopping runs" });
    await within(dialog).findByText(OTHER);
    expect(dialog).toHaveTextContent("still running a minute after it was told to stop");
    expect(within(dialog).queryByText(RUN)).not.toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Stay open" }));
    expect(screen.queryByRole("dialog", { name: "stopping runs" })).not.toBeInTheDocument();
    expect(win.destroyed).toBe(false);

    // Asked again, the dialog is back; Close anyway closes.
    await close(win);
    fireEvent.click(screen.getByRole("button", { name: "Stop all" }));
    fireEvent.click(await screen.findByRole("button", { name: "Close anyway" }));
    await waitFor(() => expect(win.destroyed).toBe(true));
    expect(stop.stops).toBe(2);
  });

  it("shows why the server could not stop the runs", async () => {
    let release = () => {};
    const held = new Promise<void>((resolve) => (release = resolve));
    const stop: FakeStop = { running: [{ run: RUN, error: null }], choice: "stop", held };
    const win = await openApp(stop);
    await close(win);
    // The stop, held until now, fails where the dry run before it did not.
    stop.error = "cannot read the runs";
    release();
    const dialog = await screen.findByRole("dialog", { name: "stopping runs" });
    expect(await within(dialog).findByText("Could not stop the runs: cannot read the runs")).toBeInTheDocument();
    expect(win.destroyed).toBe(false);
  });
});

describe("the settings", () => {
  it("show the saved close choice, and save the one picked", async () => {
    const stop: FakeStop = { choice: "stop" };
    await openApp(stop);
    fireEvent.click(await screen.findByRole("button", { name: "Settings…" }));
    const dialog = screen.getByRole("dialog", { name: "settings" });
    await waitFor(() => expect(within(dialog).getByRole("radio", { name: "Stop them" })).toBeChecked());
    fireEvent.click(within(dialog).getByRole("radio", { name: "Ask each time" }));
    await waitFor(() => expect(stop.choice).toBe("ask"));
    expect(within(dialog).getByRole("radio", { name: "Ask each time" })).toBeChecked();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog", { name: "settings" })).not.toBeInTheDocument();
  });

  it("are not offered while the app's runs are in herdr", async () => {
    await openApp({ inHerdr: true });
    expect(screen.getByRole("button", { name: "Workflows…" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Settings…" })).not.toBeInTheDocument();
  });

  it("show a choice that could not be read", async () => {
    await openApp({ choiceError: "permission denied" });
    fireEvent.click(await screen.findByRole("button", { name: "Settings…" }));
    const dialog = screen.getByRole("dialog", { name: "settings" });
    expect(await within(dialog).findByText("Could not read the setting: permission denied")).toBeInTheDocument();
  });
});
