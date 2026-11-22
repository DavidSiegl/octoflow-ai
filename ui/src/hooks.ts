import { type RefObject, useEffect, useLayoutEffect, useReducer, useRef, useState } from "react";
import { EVENT_KINDS, type Api, type Project, type RunEvent } from "./api";
import { applyEvent, NO_RUNS, type RunsState } from "./runs";

type Action = { type: "reset" } | { type: "event"; event: RunEvent; at: number };

function reduce(state: RunsState, action: Action): RunsState {
  return action.type === "reset" ? NO_RUNS : applyEvent(state, action.event, action.at);
}

/** The project's runs, kept up to date by the server's event stream for as long as the component is mounted.
 *
 * The stream starts with every run's record and files, also after the EventSource reconnects, so each opening
 * starts from no runs. A stream that cannot be opened shows as the state's problem until it reconnects.
 */
export function useRunStream(api: Api, project: Project): RunsState {
  const [state, dispatch] = useReducer(reduce, NO_RUNS);
  useEffect(() => {
    dispatch({ type: "reset" });
    const source = new EventSource(api.eventsUrl(project));
    source.addEventListener("open", () => dispatch({ type: "reset" }));
    source.addEventListener("error", () => {
      const event: RunEvent = { kind: "problem", error: "lost the connection to the orchestrator; retrying" };
      dispatch({ type: "event", event, at: Date.now() });
    });
    for (const kind of EVENT_KINDS) {
      source.addEventListener(kind, (message: MessageEvent<string>) => {
        const event = { kind, ...JSON.parse(message.data) } as RunEvent;
        dispatch({ type: "event", event, at: Date.now() });
      });
    }
    return () => source.close();
  }, [api, project]);
  return state;
}

/** The time, in milliseconds since the epoch, updated every intervalMs. */
export function useNow(intervalMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(timer);
  }, [intervalMs]);
  return now;
}

/** How far from its end, in pixels, a scrolled element still counts as at its end: the slack for the fraction of a
 * pixel that a zoomed or high-density screen can leave between the two. Small enough that the least scroll up a
 * reader can make leaves the end. */
const AT_END_SLACK_PX = 2;

/** A ref for a scrollable element that follows its end, as a terminal does, while the reader is there: each time
 * content changes, the element is scrolled to its end if it was at its end before the change, and left where the
 * reader put it otherwise. Only the element scrolls, never the page around it. It starts at its end, and again
 * whenever origin changes: content that starts over, such as a new session's, owes nothing to where the reader was in
 * the old. */
export function useFollowEnd<T extends HTMLElement>(content: unknown, origin?: unknown): RefObject<T | null> {
  const ref = useRef<T>(null);
  // The origin at the last change, and the element's scrollHeight then: where its end was before the content now
  // shown. null before the first change.
  const last = useRef<{ origin: unknown; height: number } | null>(null);
  // Before paint, so the new content never shows at the old position first. The reader's position is read here, at
  // the change, rather than kept from scroll events, which arrive a frame late.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) {
      return;
    }
    const before = last.current;
    if (!before || before.origin !== origin || before.height - el.scrollTop - el.clientHeight <= AT_END_SLACK_PX) {
      el.scrollTop = el.scrollHeight;
    }
    last.current = { origin, height: el.scrollHeight };
  }, [content, origin]);
  return ref;
}
