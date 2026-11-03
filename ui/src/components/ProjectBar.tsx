import { useState, type FormEvent } from "react";
import type { Project } from "../api";

interface Props {
  /** The project open now. */
  project: Project;
  /** Gets the project the fields name when the form is submitted. */
  onOpen: (project: Project) => void;
  /** Called by "Check setup…", which checks the project open now, not what the fields hold. */
  onCheckSetup: () => void;
}

/** The project folder and machine fields, and the button that checks the open project's setup. */
export function ProjectBar({ project, onOpen, onCheckSetup }: Props) {
  const [cwd, setCwd] = useState(project.cwd);
  const [machine, setMachine] = useState(project.machine);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    onOpen({ cwd: cwd.trim(), machine: machine.trim() });
  };
  return (
    <form className="project-bar" onSubmit={submit}>
      <label>
        Project folder
        <input value={cwd} onChange={(e) => setCwd(e.target.value)} placeholder="~/GitHub/myproject" />
      </label>
      <label>
        Machine
        <input value={machine} onChange={(e) => setMachine(e.target.value)} placeholder="this one" />
      </label>
      <button type="submit" disabled={!cwd.trim()}>
        Open
      </button>
      <button type="button" onClick={onCheckSetup} disabled={!project.cwd}>
        Check setup…
      </button>
    </form>
  );
}
