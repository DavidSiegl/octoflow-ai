import type { ReactNode } from "react";
import type { Project } from "../api";

interface Props {
  /** The dialog's accessible name. */
  label: string;
  title: string;
  /** The project the dialog is about, shown under the title. */
  project: Project;
  /** A class for the box besides `dialog`, for what its content needs. */
  className: string;
  children: ReactNode;
}

/** A modal box over the rest of the window: its title, the project it is about, and children. A dialog rendered
 * later in the document is in front of one rendered earlier. */
export function Dialog({ label, title, project, className, children }: Props) {
  return (
    <div className="dialog-backdrop">
      <section className={`dialog ${className}`} role="dialog" aria-modal="true" aria-label={label}>
        <h2>{title}</h2>
        <p className="where">
          {project.cwd} {project.machine ? `on ${project.machine}` : "on this computer"}
        </p>
        {children}
      </section>
    </div>
  );
}
