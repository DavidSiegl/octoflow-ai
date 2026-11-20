import { useEffect, useState } from "react";
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

/** Where the close dialog is: asking, with the runs running when the window was asked to close; stopping them; or
 * done stopping, with the runs that failed to stop, or the error that kept the server from stopping any. */
type Closing =
  | { step: "ask"; running: StoppedRun[] }
  | { step: "stopping"; running: StoppedRun[] }
  | { step: "failed"; failed: StoppedRun[] | string };

/** Guards the window's close while the project has "acp" runs running (Api.previewStop): it asks whether to keep them
 * running or stop them, unless a choice was saved with "Don't ask again" (Api.closeChoice), and then closes the
 * window. Runs in herdr, which the app never stops, do not keep it from closing at once; nor does a server that cannot
 * be asked, which could not stop a run either. A run keeps running whether or not the app does. */
export function CloseGuard({ api, project, appWindow }: { api: Api; project: Project; appWindow: AppWindow }) {
  const [closing, setClosing] = useState<Closing | null>(null);
  useEffect(() => {
    let current = true;
    const listening = appWindow.onCloseRequested(async (event) => {
      if (!project.cwd) {
        return;
      }
      let running: StoppedRun[];
      let choice: string;
      try {
        [running, choice] = await Promise.all([api.previewStop(project), api.closeChoice()]);
      } catch {
        return;
      }
      if (running.length === 0 || choice === "keep") {
        return;
      }
      event.preventDefault();
      if (current) {
        setClosing({ step: choice === "stop" ? "stopping" : "ask", running });
      }
    });
    return () => {
      current = false;
      void listening.then((stop) => stop());
    };
  }, [api, project, appWindow]);

  // A saved "stop" goes straight to stopping, as does Stop all.
  const stopping = closing?.step === "stopping";
  useEffect(() => {
    if (!stopping) {
      return;
    }
    api.stopRuns(project).then(
      (stopped) => {
        const failed = stopped.filter((r) => r.error !== null);
        if (failed.length === 0) {
          void appWindow.destroy();
        } else {
          setClosing({ step: "failed", failed });
        }
      },
      (error: Error) => setClosing({ step: "failed", failed: error.message }),
    );
  }, [stopping, api, project, appWindow]);

  if (!closing) {
    return null;
  }
  const close = () => void appWindow.destroy();
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
        <p className="problem">Could not stop the runs: {closing.failed}</p>
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
 * be saved still goes ahead: the next close asks again. */
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
  const choose = (choice: "keep" | "stop", then: () => void) => {
    if (!remember) {
      return then();
    }
    api.saveCloseChoice(choice).then(then, then);
  };
  return (
    <Dialog label="close the app" title="Close the app?" project={project} className="close-app">
      <p>
        {count(running.length)} {running.length === 1 ? "is" : "are"} running: {running.map((r) => r.run).join(", ")}.
        They go on without the app, and show here again when it opens; or stop them now, to resume later.
      </p>
      <label className="check">
        <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
        Don't ask again
      </label>
      <footer>
        <button type="button" onClick={() => choose("stop", onStop)}>
          Stop all
        </button>
        <button type="button" onClick={() => choose("keep", onKeep)} autoFocus>
          Keep running
        </button>
      </footer>
    </Dialog>
  );
}

function count(runs: number): string {
  return runs === 1 ? "1 run" : `${runs} runs`;
}
