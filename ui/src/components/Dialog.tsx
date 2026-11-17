import { useLayoutEffect, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import type { Project } from "../api";

interface Props {
  /** The dialog's accessible name. */
  label: string;
  title: string;
  /** The project the dialog is about, shown under the title; none for a dialog about no project. */
  project?: Project;
  /** A class for the box besides `dialog`, for what its content needs. */
  className: string;
  children: ReactNode;
}

/** A modal box over the rest of the window: its title, the project it is about if any, and children. While it is
 * open, everything else in the window is inert, unreachable by Tab, clicks and keys, so that nothing behind it, such
 * as the project bar, can change what it is about. A dialog opened later is in front of one opened earlier, which is
 * inert until the later one closes. */
export function Dialog({ label, title, project, className, children }: Props) {
  // The child of document.body the dialog is rendered into, outside the app's root, so that the root can be inert.
  const [host] = useState(() => document.createElement("div"));
  useLayoutEffect(() => {
    document.body.append(host);
    open.push(host);
    settleInert();
    return () => {
      open.splice(open.indexOf(host), 1);
      host.remove();
      madeInert.delete(host);
      settleInert();
    };
  }, [host]);
  return createPortal(
    <div className="dialog-backdrop">
      <section className={`dialog ${className}`} role="dialog" aria-modal="true" aria-label={label}>
        <h2>{title}</h2>
        {project && (
          <p className="where">
            {project.cwd} {project.machine ? `on ${project.machine}` : "on this computer"}
          </p>
        )}
        {children}
      </section>
    </div>,
    host,
  );
}

// The open dialogs' hosts, in the order they opened: the last is in front.
const open: HTMLElement[] = [];
// The children of document.body that are inert because a dialog is in front of them, not because anything else made
// them so.
const madeInert = new Set<Element>();

// Makes every child of document.body but the front dialog's host inert, and lets go of those it made inert once no
// dialog is in front of them.
function settleInert() {
  const front = open.at(-1);
  for (const child of document.body.children) {
    if (front && child !== front) {
      if (!child.hasAttribute("inert")) {
        child.setAttribute("inert", "");
        madeInert.add(child);
      }
    } else if (madeInert.delete(child)) {
      child.removeAttribute("inert");
    }
  }
}
