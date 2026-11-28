import { useCallback, useEffect, useRef, useState } from "react";
import type { Api, Project, StoppedRun } from "../api";
import { Dialog } from "./Dialog";

/** What the app needs of its window: to hear that the user asked to close it, and to close it. Tauri's Window is one
 * (main.tsx). */
export interface AppWindow {
  /** Call handler each time the user asks to close the window, then close it unless handler called preventDefault
   * before its promise settled; resolves to a function that stops calling it. */
  onCloseRequested(handler: (event: { preventDefault(): void }) => Promise<void>): Promise<() => void>;
  /** Close the window, without asking handler again; the app exits with it. */
  destroy(): Promise<void>;
}

/** How long, in milliseconds, a close waits to learn whether runs are running before it closes anyway, as it does
 * when the server cannot be asked, and how long the close dialog waits for its choice to be saved before acting on
 * it: the window cannot close while it waits. */
export const CLOSE_CHECK_WAIT = 5000;

/** Where the close dialog is: asking, with the runs running when the window was asked to close; stopping them; or
 * failed, with the runs that did not stop, or a sentence saying why nothing could be stopped or the window closed. */
type Closing =
  | { step: "ask"; running: StoppedRun[] }
  | { step: "stopping"; running: StoppedRun[] }
  | { step: "failed"; failed: StoppedRun[] | string };

/** Guards the window's close while the open project has "acp" runs running (Api.previewStop): it asks whether to
 * keep them running or stop them, unless a choice was saved with "Don't ask again" (Api.closeChoice), and then closes
 * the window. It closes at once when the choice is to keep them running, when no run is running, and when the server cannot say within CLOSE_CHECK_WAIT, since it could not stop a
 * run either. Asked to close again while it asks or stops, it carries on with that. A run keeps running whether or not
 * the app does. */
export function CloseGuard({ api, project, appWindow }: { api: Api; project: Project; appWindow: AppWindow }) {
  const [closing, setClosing] = useState<Closing | null>(null);
  // closing, for the close handler, which outlives the render it was made in.
  const closingNow = useRef<Closing | null>(null);
  useEffect(() => {
    closingNow.current = closing;
  }, [closing]);

  useEffect(() => {
    let current = true;
    const listening = appWindow.onCloseRequested(async (event) => {
      if (closingNow.current) {
        event.preventDefault();
        return;
      }
      if (!project.cwd) {
        return;
      }
      const check = async () => {
        const choice = await api.closeChoice();
        if (choice === "keep") {
          return null;
        }
        const running = await api.previewStop(project);
        return running.length === 0 ? null : { choice, running };
      };
      let found: Awaited<ReturnType<typeof check>>;
      try {
        found = await withDeadline(check(), CLOSE_CHECK_WAIT);
      } catch {
        return;
      }
      if (!found) {
        return;
      }
      event.preventDefault();
      if (current) {
        setClosing({ step: found.choice === "stop" ? "stopping" : "ask", running: found.running });
      }
    });
    return () => {
      current = false;
      void listening.then((stop) => stop());
    };
  }, [api, project, appWindow]);

  const close = useCallback(() => {
    appWindow.destroy().catch((error: Error) => {
      setClosing({ step: "failed", failed: `Could not close the window: ${error.message}` });
    });
  }, [appWindow]);

  // A saved "stop" goes straight to stopping, as does Stop all.
  const stopping = closing?.step === "stopping";
  useEffect(() => {
    if (!stopping) {
      return;
    }
    let current = true;
    api.stopRuns(project).then(
      (stopped) => {
        if (!current) {
          return;
        }
        const failed = stopped.filter((r) => r.error !== null);
        if (failed.length === 0) {
          close();
        } else {
          setClosing({ step: "failed", failed });
        }
      },
      (error: Error) => current && setClosing({ step: "failed", failed: `Could not stop the runs: ${error.message}` }),
    );
    return () => {
      current = false;
    };
  }, [stopping, api, project, close]);

  if (!closing) {
    return null;
  }
  if (closing.step === "ask") {
    return (
      <AskDialog
        api={api}
        project={project}
        running={closing.running}
        onKeep={close}
        onStop={() => setClosing({ step: "stopping", running: closing.running })}
      />
    );
  }
  return (
    <Dialog label="stopping runs" title="Stopping runs" project={project} className="close-app">
      {closing.step === "stopping" ? (
        <p className="loading">Stopping {count(closing.running.length)}…</p>
      ) : typeof closing.failed === "string" ? (
        <p className="problem">{closing.failed}</p>
      ) : (
        <ul className="checks">
          {closing.failed.map((r) => (
            <li key={r.run} data-state="missing">
              <span className="check-name">{r.run}</span> {r.error}
            </li>
          ))}
        </ul>
      )}
      {closing.step === "failed" && (
        <footer>
          <button type="button" onClick={() => setClosing(null)}>
            Stay open
          </button>
          <button type="button" onClick={close}>
            Close anyway
          </button>
        </footer>
      )}
    </Dialog>
  );
}

/** "Keep running" (focused) or "Stop all", with "Don't ask again", which saves the choice made. A choice that cannot
 * be saved, or whose save is not answered within CLOSE_CHECK_WAIT, still goes ahead: the next close asks again. Once
 * one is clicked, nothing in the dialog can be until it acts, so that one choice is saved and acted on. */
function AskDialog({
  api,
  project,
  running,
  onKeep,
  onStop,
}: {
  api: Api;
  project: Project;
  running: StoppedRun[];
  onKeep: () => void;
  onStop: () => void;
}) {
  const [remember, setRemember] = useState(false);
  // True once Stop all or Keep running was clicked: the dialog is acting on that choice, and takes no other.
  const [chosen, setChosen] = useState(false);
  const choose = (choice: "keep" | "stop", then: () => void) => {
    setChosen(true);
    if (!remember) {
      return then();
    }
    withDeadline(api.saveCloseChoice(choice), CLOSE_CHECK_WAIT).then(then, then);
  };
  return (
    <Dialog label="close the app" title="Close the app?" project={project} className="close-app">
      <p>
        {count(running.length)} {running.length === 1 ? "is" : "are"} running: {running.map((r) => r.run).join(", ")}.
        They go on without the app, and show here again when it opens; or stop them now, to resume later.
      </p>
      <label className="check">
        <input type="checkbox" checked={remember} disabled={chosen} onChange={(e) => setRemember(e.target.checked)} />
        Don't ask again
      </label>
      <footer>
        <button type="button" disabled={chosen} onClick={() => choose("stop", onStop)}>
          Stop all
        </button>
        <button type="button" disabled={chosen} onClick={() => choose("keep", onKeep)} autoFocus>
          Keep running
        </button>
      </footer>
    </Dialog>
  );
}

/** promise's value, or a rejection once ms milliseconds pass before it settles. */
function withDeadline<T>(promise: Promise<T>, ms: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`no answer within ${ms} ms`)), ms);
  });
  return Promise.race([promise, late]).finally(() => clearTimeout(timer));
}

function count(runs: number): string {
  return runs === 1 ? "1 run" : `${runs} runs`;
}
