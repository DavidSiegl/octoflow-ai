import { invoke } from "@tauri-apps/api/core";
import { useRef, useState, type ReactNode } from "react";

/** What a link opens outside the app, as the Tauri shell's `open_outside` takes it (`Outside`,
 * ui/src-tauri/src/lib.rs): a web page by its `https` URL, which the shell refuses otherwise, or a third-party license
 * file a packaged app bundles, by its name. */
export type Outside = { page: string } | { license: "crate-licenses" | "npm-licenses" };

/** A link that opens target outside the app, in the system's browser or HTML viewer, and does nothing else: the
 * click goes no further, so a link inside a clickable row does not also select the row. It is a button drawn as a
 * link, since a webview could follow an `<a href>` itself, into the app's own window. Why the last click could not
 * open it shows beside it, until the next click. title is the button's tooltip. */
export function OutsideLink({ target, title, children }: { target: Outside; title?: string; children: ReactNode }) {
  // Why the click last made could not open target; null while it has not failed.
  const [failure, setFailure] = useState<string | null>(null);
  // How many clicks have been made: an open that fails after a later click says nothing.
  const clicks = useRef(0);
  const open = (event: React.MouseEvent) => {
    event.stopPropagation();
    const click = ++clicks.current;
    setFailure(null);
    invoke("open_outside", { target }).catch((error: unknown) => {
      if (click === clicks.current) setFailure(String(error));
    });
  };
  return (
    <>
      <button type="button" className="link" title={title} onClick={open}>
        {children}
      </button>
      {failure && <span className="problem"> Could not open it: {failure}</span>}
    </>
  );
}
