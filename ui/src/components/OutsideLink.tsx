import { invoke } from "@tauri-apps/api/core";
import { useState, type ReactNode } from "react";

/** What a link opens outside the app, as the Tauri shell's `open_outside` takes it (`Outside`,
 * ui/src-tauri/src/lib.rs): a web page by its `https` URL, which the shell refuses otherwise, or a third-party license
 * file a packaged app bundles, by its name. */
export type Outside = { page: string } | { license: "crate-licenses" | "npm-licenses" };

// How many OutsideLinks have been clicked, in any of them: an open that fails after a later click, of this link or
// another, says nothing.
let clicks = 0;

/** A link that opens target outside the app, in the system's browser or HTML viewer, and does nothing else: a click
 * on it, or on its failure, goes no further, so a link inside a clickable row does not also select the row. It is a
 * button drawn as a link, since a webview could follow an `<a href>` itself, into the app's own window. Why its last
 * click could not open it shows beside it until it is clicked again, unless some OutsideLink was clicked after.
 * title is the button's tooltip. */
export function OutsideLink({ target, title, children }: { target: Outside; title?: string; children: ReactNode }) {
  // Why this link's last click could not open target; null while it has not failed.
  const [failure, setFailure] = useState<string | null>(null);
  const open = () => {
    const click = ++clicks;
    setFailure(null);
    invoke("open_outside", { target }).catch((error: unknown) => {
      if (click === clicks) setFailure(String(error));
    });
  };
  return (
    // Not interactive itself: it only keeps its clicks from the row or element around it.
    <span className="outside-link" onClick={(event) => event.stopPropagation()}>
      <button type="button" className="link" title={title} onClick={open}>
        {children}
      </button>
      {failure && <span className="problem"> Could not open it: {failure}</span>}
    </span>
  );
}
