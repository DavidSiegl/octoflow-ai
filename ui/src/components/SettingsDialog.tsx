import { useEffect, useState } from "react";
import type { Api, CloseChoice } from "../api";
import { Dialog } from "./Dialog";

const CHOICES: { choice: CloseChoice; label: string }[] = [
  { choice: "ask", label: "Ask each time" },
  { choice: "keep", label: "Keep them running" },
  { choice: "stop", label: "Stop them" },
];

/** The app's settings, this computer's for every project: what closing the window does with runs still running
 * (CloseGuard), which "Don't ask again" in its dialog sets. A choice is saved as soon as it is picked; onClose is
 * called on Close or Escape. */
export function SettingsDialog({ api, onClose }: { api: Api; onClose: () => void }) {
  // The saved choice; null while it is read, or when it could not be.
  const [choice, setChoice] = useState<CloseChoice | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    api.closeChoice().then(
      (saved) => current && setChoice(saved),
      (error: Error) => current && setProblem(`Could not read the setting: ${error.message}`),
    );
    return () => {
      current = false;
    };
  }, [api]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => event.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const pick = (next: CloseChoice) => {
    const before = choice;
    setChoice(next);
    setProblem(null);
    api.saveCloseChoice(next).catch((error: Error) => {
      setChoice(before);
      setProblem(`Could not save the setting: ${error.message}`);
    });
  };
  return (
    <Dialog label="settings" title="Settings" className="settings">
      <fieldset disabled={choice === null && problem === null}>
        <legend>When the app closes with runs running</legend>
        {CHOICES.map(({ choice: value, label }) => (
          <label key={value} className="check">
            <input type="radio" name="close-choice" checked={choice === value} onChange={() => pick(value)} />
            {label}
          </label>
        ))}
      </fieldset>
      {problem && <p className="problem">{problem}</p>}
      <footer>
        <button type="button" onClick={onClose} autoFocus>
          Close
        </button>
      </footer>
    </Dialog>
  );
}
