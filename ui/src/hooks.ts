import { type RefObject, useEffect, useLayoutEffect, useReducer, useRef, useState } from "react";
import { EVENT_KINDS, type Api, type Project, type RunEvent } from "./api";
import { applyEvent, NO_RUNS, relisting, type RunsState } from "./runs";

type Action = { type: "reset" } | { type: "open" } | { type: "event"; event: RunEvent; at: number };

function reduce(state: RunsState, action: Action): RunsState {
  switch (action.type) {
    case "reset":
      return NO_RUNS;
    case "open":
      return relisting(state);
    case "event":
      return applyEvent(state, action.event, action.at);
  }
}

/** The project's runs, kept up to date by the server's event stream for as long as the component is mounted.
 *
 * The stream starts with every run's record and files, then `listed`, also after the EventSource reconnects. A
 * project starts from no runs; a reconnect keeps the runs shown, so that an open run's detail stays mounted, until
 * listed drops those the new stream did not tell of (RunsState.relisted). A dropped stream, which the EventSource reconnects, shows as the state's problem until it
 * does. A stream the server refused, as for a project folder it cannot open, is closed for good: its problem is then
 * the server's reason (Api.runsProblem), and nothing reconnects it but opening the project again.
 */
export function useRunStream(api: Api, project: Project): RunsState {
  const [state, dispatch] = useReducer(reduce, NO_RUNS);
  useEffect(() => {
    let current = true;
    // A problem to show, unless the effect has been cleaned up since: the server's reason arrives after a request.
    const problem = (error: string) =>
      current && dispatch({ type: "event", event: { kind: "problem", error }, at: Date.now() });
    dispatch({ type: "reset" });
    const source = new EventSource(api.eventsUrl(project));
    source.addEventListener("open", () => dispatch({ type: "open" }));
    source.addEventListener("error", () => {
      if (source.readyState !== EventSource.CLOSED) {
        problem("lost the connection to the orchestrator; retrying");
        return;
      }
      api.runsProblem(project).then(
        (why) =>
          problem(`${why ?? "the orchestrator closed the project's event stream"}; open the project again to retry`),
        (error: Error) => problem(`lost the connection to the orchestrator: ${error.message}`),
      );
    });
    for (const kind of EVENT_KINDS) {
      source.addEventListener(kind, (message: MessageEvent<string>) => {
        const event = { kind, ...JSON.parse(message.data) } as RunEvent;
        dispatch({ type: "event", event, at: Date.now() });
      });
    }
    return () => {
      current = false;
      source.close();
    };
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

/** Whether el is scrolled to its end, within AT_END_SLACK_PX, for content height pixels high: by default what it
 * holds now; useFollowEnd passes the height before a change, to ask where the reader was. */
function atEnd(el: HTMLElement, height = el.scrollHeight): boolean {
  return height - el.scrollTop - el.clientHeight <= AT_END_SLACK_PX;
}

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
    if (!before || before.origin !== origin || atEnd(el, before.height)) {
      el.scrollTop = el.scrollHeight;
    }
    last.current = { origin, height: el.scrollHeight };
  }, [content, origin]);
  return ref;
}

/** For a scrollable element whose content each change replaces whole, such as the last lines of a terminal, so that
 * new content would slide what the reader is on out from under them: the content to show, which holds while the reader
 * is scrolled up from the element's end. shown is latest while the element is at its end, and the element follows its
 * end as useFollowEnd does; once the reader scrolls up, shown stays what it was, through every change of latest, until
 * they scroll back to the end, which shows latest. held is true while shown is not latest. toEnd scrolls the element
 * to its end and shows latest, as the reader's own scroll there would. ref goes on the element, which must be mounted
 * from the hook's first render to its last: the scroll listener is attached to it once. */
export function useHeldWhileScrolledUp<T extends HTMLElement, C>(
  latest: C,
): { ref: RefObject<T | null>; shown: C; held: boolean; toEnd: () => void } {
  const [shown, setShown] = useState(latest);
  const ref = useFollowEnd<T>(shown);
  // latest as of the last render, for the scroll listener, which outlives renders.
  const latestNow = useRef(latest);
  // Before paint, as useFollowEnd's: whether the reader is at the end is read from the element when latest changes,
  // not kept from scroll events, which arrive a frame late.
  useLayoutEffect(() => {
    latestNow.current = latest;
    const el = ref.current;
    if (!el || atEnd(el)) {
      setShown(latest);
    }
  }, [latest, ref]);
  useEffect(() => {
    const el = ref.current;
    if (!el) {
      return;
    }
    const onScroll = () => {
      if (atEnd(el)) {
        setShown(latestNow.current);
      }
    };
    el.addEventListener("scroll", onScroll);
    return () => el.removeEventListener("scroll", onScroll);
  }, [ref]);
  const toEnd = () => {
    const el = ref.current;
    if (el) {
      el.scrollTop = el.scrollHeight;
    }
    setShown(latestNow.current);
  };
  return { ref, shown, held: shown !== latest, toEnd };
}
