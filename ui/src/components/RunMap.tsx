import { useEffect, useRef, useState } from "react";
import type { RunRecord, Turn } from "../api";
import { formatAge, formatRound, prLabel, progress, type Stage, type StageState } from "../runs";
import { along, armOutline, armSlots, armTip, HEAD, MAP_HEIGHT, MAP_WIDTH, spine, type ArmPose } from "../runMap";

interface Props {
  record: RunRecord;
  /** The time, in milliseconds since the epoch, a running turn's length is counted to. */
  now: number;
}

/** The run as an octopus: the head is the run, each stage of its progress an arm ending in a station, left to right,
 * with its verdict steps' loops drawn below; then a line on what runs now and the run's turns, newest first. Moves
 * unless the viewer prefers reduced motion, and says everything at rest. */
export function RunMap({ record, now }: Props) {
  const stages = progress(record);
  const svg = useRef<SVGSVGElement>(null);
  const reduced = useReducedMotion();
  const stale = record.health === "stale";
  const waiting = record.waiting && record.waiting.reason !== "stale" ? record.waiting.reason : null;
  const arms = armSlots(stages.length).map((slot) => {
    const stage = slot.stage === null ? null : (stages[slot.stage] ?? null);
    return { ...slot, stage, motion: armMotion(stage?.state ?? null, stale) };
  });
  const working = arms.find((arm) => arm.stage?.state === "current");
  // The animation reads the latest render's arms each frame, so a new record changes the motion without restarting it.
  const scene = useRef<Scene | null>(null);
  useEffect(() => {
    const map = svg.current;
    scene.current = {
      arms: arms.map((arm, i) => {
        const path = map?.querySelectorAll<SVGPathElement>("path[data-arm]")[i] ?? null;
        const suckers = [...(path?.parentElement?.querySelectorAll("circle[data-sucker]") ?? [])];
        return { angle: arm.angle, motion: arm.motion, path, suckers };
      }),
      working: working && !stale ? [working.angle, working.motion] : null,
      back: waiting !== null,
      bob: !stale,
      head: map?.querySelector("g[data-head]") ?? null,
      pulses: [...(map?.querySelectorAll("circle[data-pulse]") ?? [])],
    };
  });

  useEffect(() => {
    if (reduced) {
      return;
    }
    let frame = requestAnimationFrame(function draw(time) {
      frame = requestAnimationFrame(draw);
      const current = scene.current;
      if (!current) {
        return;
      }
      current.arms.forEach(({ angle, motion, path, suckers }) => {
        const shape = armShape(angle, motion, time);
        path?.setAttribute("d", shape.outline);
        suckers.forEach((sucker, k) => {
          const at = shape.suckers[k];
          sucker.setAttribute("cx", at?.x.toFixed(1) ?? "0");
          sucker.setAttribute("cy", at?.y.toFixed(1) ?? "0");
        });
      });
      const bob = current.bob ? (Math.sin(time / 700) * 2.5).toFixed(2) : "0";
      current.head?.setAttribute("transform", `translate(0 ${bob})`);
      const { pulses, working, back } = current;
      pulses.forEach((pulse, k) => {
        if (!working) {
          return;
        }
        // Work handed out runs from the head to the working tip; a wait on the human runs back to the head.
        const [angle, motion] = working;
        const t = (time / 1400 + k / pulses.length) % 1;
        const at = along(spine(angle, { ...motion.pose, sway: sway(motion, time) }), back ? 1 - t : t);
        pulse.setAttribute("cx", at.x.toFixed(1));
        pulse.setAttribute("cy", at.y.toFixed(1));
        pulse.setAttribute("opacity", (Math.sin(t * Math.PI) * 0.9 + 0.1).toFixed(2));
      });
    });
    return () => cancelAnimationFrame(frame);
  }, [reduced]);

  const stationAt = (stage: Stage) => {
    const arm = arms.find((a) => a.stage === stage);
    return arm ? armTip(arm.angle, arm.motion.pose.reach) : HEAD;
  };
  const loops = stages.flatMap((stage) => {
    const target = stages.find((st) => st.kind === "step" && st.step === stage.loopTo);
    if (stage.loopTo === null || !target) {
      return [];
    }
    const taken = (record.verdicts ?? []).some((v) => v.step === stage.step && v.verdict !== "APPROVE");
    return [{ from: stationAt(stage), to: stationAt(target), step: stage.step, loopTo: stage.loopTo, taken }];
  }).sort((a, b) => a.from.y - b.from.y);
  const loopFloor = Math.max(...loops.flatMap((loop) => [loop.from.y, loop.to.y])) + 62 + 40;
  const tips = stages.map(stationAt);
  const face = record.error ? "failed" : stale ? "asleep" : "awake";
  // A failed run failed in its phase: the newest turn of that phase is where, if one started (quality's turns are
  // saved under the phase too).
  const failedTurn = record.error ? [...record.turns].reverse().find((turn) => turn.step === record.phase) : undefined;
  // The pupils look toward the working arm, or straight out at the viewer the run waits on.
  const look = waiting || !working ? { x: 0, y: 2 } : { x: Math.cos(working.angle) * 4, y: Math.sin(working.angle) * 4 };

  return (
    <div className="run-map">
      <svg
        ref={svg}
        viewBox={`0 0 ${MAP_WIDTH} ${MAP_HEIGHT}`}
        role="list"
        aria-label="stages"
        className={stale ? "stale" : undefined}
      >
        <defs>
          <radialGradient id="run-map-glow">
            <stop offset="0" className="glow-centre" />
            <stop offset="1" className="glow-edge" />
          </radialGradient>
          <marker id="run-map-loop-head" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto">
            <path d="M0,0 L10,5 L0,10 z" className="loop-head" />
          </marker>
        </defs>
        <g aria-hidden="true">
          {arms.map((arm, i) => {
            const shape = armShape(arm.angle, arm.motion, 0);
            const armState = arm.stage ? arm.stage.state : "spare";
            return (
              <g key={i} className={`arm ${armState}`}>
                <path data-arm="" d={shape.outline} />
                {(armState === "done" || armState === "again") &&
                  shape.suckers.map((at, k) => <circle key={k} data-sucker="" className="sucker" cx={at.x} cy={at.y} r={2.4} />)}
              </g>
            );
          })}
        </g>
        <g aria-hidden="true">
          {loops.map((loop, k) => {
            // Each loop dips below every station's text, from under its verdict station to under the step it goes
            // back to, and is named at its lowest point. The loops are ordered by how high their verdict station
            // sits, so each further one, dipping deeper from lower down, has its name clearly below the last's.
            const [y0, y1] = [loop.from.y + 62, loop.to.y + 62];
            const control = { x: (loop.from.x + loop.to.x) / 2, y: loopFloor + 30 * k };
            const bottom = 0.25 * (y0 + y1) + 0.5 * control.y;
            return (
              <g key={`${loop.step}-${loop.loopTo}`} className={`loop ${loop.taken ? "taken" : ""}`}>
                <path
                  d={`M${loop.from.x},${y0} Q${control.x},${control.y} ${loop.to.x},${y1}`}
                  markerEnd="url(#run-map-loop-head)"
                />
                <text x={control.x} y={bottom + 12} textAnchor="middle">
                  {loop.taken ? `changes → ${loop.loopTo}` : `loops to ${loop.loopTo}`}
                </text>
              </g>
            );
          })}
        </g>
        <g data-head="" className={`head ${face}`} aria-hidden="true">
          {working && !stale && !record.error && <circle className="glow" cx={HEAD.x} cy={HEAD.y + 10} r={95} />}
          <path className="mantle" d={MANTLE} />
          <ellipse className="shine" cx={HEAD.x - 30} cy={HEAD.y - 70} rx={10} ry={6} />
          {[-22, 22].map((dx) => (
            <Eye key={dx} x={HEAD.x + dx} y={HEAD.y + 12} face={face} look={look} />
          ))}
          <text className="round" x={HEAD.x} y={HEAD.y - 48} textAnchor="middle">
            {record.phase === "done" ? "done" : formatRound(record)}
          </text>
          <text className="key" x={HEAD.x} y={HEAD.y - 26} textAnchor="middle">
            {record.key}
          </text>
          {stale && (
            <text className="snore" x={HEAD.x + 52} y={HEAD.y - 70}>
              z z
            </text>
          )}
          {waiting && (
            <g className="bubble">
              <path d={BUBBLE} />
              <text x={HEAD.x + 104} y={HEAD.y - 70} textAnchor="middle">
                {waiting === "interview" ? "your answer?" : "blocked"}
              </text>
            </g>
          )}
        </g>
        {stages.map((stage, i) => (
          <Station
            key={`${stage.kind}:${stage.step ?? stage.label}`}
            stage={stage}
            record={record}
            waiting={waiting !== null}
            at={tips[i] ?? HEAD}
            room={textRoom(tips, i)}
          />
        ))}
        {!reduced && working && !stale && (
          <g aria-hidden="true" className={`pulses ${waiting ? "back" : ""}`}>
            {[0, 1, 2].map((k) => (
              <circle key={k} data-pulse="" r={4} cx={HEAD.x} cy={HEAD.y} opacity={0} />
            ))}
          </g>
        )}
      </svg>
      <p className="now">{nowLine(record, now)}</p>
      <ol className="feed" aria-label="turns">
        {[...record.turns].reverse().map((turn) => (
          <FeedRow
            key={`${turn.file}-${turn.started_at}`}
            turn={turn}
            state={turn === failedTurn ? "failed" : turn.ended_at !== null ? "ended" : runDriven(record) ? "running" : "unfinished"}
          />
        ))}
      </ol>
    </div>
  );
}

/** What the animation draws, as of the latest render: each arm by its angle and motion, left to right, with its
 * outline and suckers (null and [] before the map is mounted); the working arm, which carries the pulses, or null when
 * none works or the run is stale; whether the pulses run back to the head, the run waiting on the human; whether the
 * head bobs, as it does unless the run is stale; and the head and pulse elements. */
interface Scene {
  arms: { angle: number; motion: ArmMotion; path: SVGPathElement | null; suckers: Element[] }[];
  working: [number, ArmMotion] | null;
  back: boolean;
  bob: boolean;
  head: Element | null;
  pulses: Element[];
}

/** How an arm moves: its pose at rest, and its sway's size (viewBox units), period (ms) and phase (radians). */
interface ArmMotion {
  pose: ArmPose;
  sway: number;
  period: number;
  phase: number;
}

/** How the arm of a stage in state (null for a spare arm) lies and moves: a spare arm curls short, a stage to do
 * hangs a little short, and the working arm sways the most; a stale run's arms barely move. */
function armMotion(state: StageState | null, stale: boolean): ArmMotion {
  const reach = state === null ? 105 : state === "todo" ? 198 : 230;
  const curl = state === null ? 46 : state === "todo" ? 14 : 0;
  const swing = stale ? 1.5 : state === "current" ? 9 : state === null ? 6 : 3;
  return { pose: { reach, curl, sway: 0 }, sway: swing, period: state === "current" ? 380 : 900, phase: reach + curl };
}

/** An arm's sway at time (ms); 0 at time 0, so the first frame is the arm at rest. */
function sway(motion: ArmMotion, time: number): number {
  return time === 0 ? 0 : Math.sin(time / motion.period + motion.phase) * motion.sway;
}

/** The arm at angle as drawn at time: its outline, and the six suckers a resting arm shows along it. */
function armShape(angle: number, motion: ArmMotion, time: number) {
  const points = spine(angle, { ...motion.pose, sway: sway(motion, time) });
  const spare = motion.pose.reach < 150;
  const outline = armOutline(points, spare ? 8 : 11, spare ? 1.5 : 3.5);
  const suckers = Array.from({ length: 6 }, (_, i) => {
    const t = 0.2 + i * 0.12;
    const [p, q] = [along(points, t), along(points, t + 0.01)];
    const d = Math.hypot(q.x - p.x, q.y - p.y) || 1;
    const w = 5 * (1 - t * 0.6) * 0.4;
    return { x: p.x + ((q.y - p.y) / d) * w, y: p.y - ((q.x - p.x) / d) * w };
  });
  return { outline, suckers };
}

/** Whether the viewer asks for less motion, kept current as they change the setting; false where the browser cannot
 * say (as in jsdom). */
function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() => reducedMotionQuery()?.matches ?? false);
  useEffect(() => {
    const query = reducedMotionQuery();
    if (!query) {
      return;
    }
    const changed = () => setReduced(query.matches);
    query.addEventListener("change", changed);
    return () => query.removeEventListener("change", changed);
  }, []);
  return reduced;
}

function reducedMotionQuery(): MediaQueryList | null {
  return typeof window.matchMedia === "function" ? window.matchMedia("(prefers-reduced-motion: reduce)") : null;
}

/** The width, in viewBox units, the texts centred under the station at tips[i] may take without reaching a
 * neighbour's or the map's edge: the gap to the nearest station whose texts sit at about the same height, and twice
 * the distance to the nearer side, less a margin. */
function textRoom(tips: { x: number; y: number }[], i: number): number {
  const tip = tips[i] ?? HEAD;
  const gaps = tips.filter((other, j) => j !== i && Math.abs(other.y - tip.y) < 60).map((other) => Math.abs(other.x - tip.x));
  return Math.min(2 * tip.x, 2 * (MAP_WIDTH - tip.x), ...gaps) - 4;
}

/** text cut to fit width viewBox units at a font size of px, ending in "…" when cut; a character is taken as 0.6em
 * wide, as Roboto Mono's are and wider than most of Roboto's. */
function fit(text: string, width: number, px: number): string {
  const most = Math.max(1, Math.floor(width / (px * 0.6)));
  return text.length <= most ? text : `${text.slice(0, most - 1)}…`;
}

const { x: HX, y: HY } = HEAD;
/** The head's outline: a mantle, rounded on top, over a flatter lower edge where the arms grow. */
const MANTLE = `M${HX - 58},${HY + 42} C${HX - 70},${HY - 40} ${HX - 40},${HY - 92} ${HX},${HY - 92}
  C${HX + 40},${HY - 92} ${HX + 70},${HY - 40} ${HX + 58},${HY + 42}
  C${HX + 40},${HY + 58} ${HX - 40},${HY + 58} ${HX - 58},${HY + 42} Z`;
/** A speech bubble up and to the right of the head, its tail pointing down at it. */
const BUBBLE = `M${HX + 52},${HY - 96} h96 a10,10 0 0 1 10,10 v22 a10,10 0 0 1 -10,10 h-78 l-14,12 v-12 h-4
  a10,10 0 0 1 -10,-10 v-22 a10,10 0 0 1 10,-10z`;

/** One eye at (x, y): open with its pupil offset by look, closed while the run is asleep (stale), crossed out when it
 * failed. */
function Eye({ x, y, face, look }: { x: number; y: number; face: string; look: { x: number; y: number } }) {
  if (face === "asleep") {
    return <path className="lid" d={`M${x - 9},${y} q9,7 18,0`} />;
  }
  if (face === "failed") {
    return <path className="lid" d={`M${x - 7},${y - 7} l14,14 M${x + 7},${y - 7} l-14,14`} />;
  }
  return (
    <>
      <circle className="eye" cx={x} cy={y} r={11} />
      <circle className="pupil" cx={x + look.x} cy={y + look.y} r={5.5} />
    </>
  );
}

/** What a reader hears for each state, after the stage's name. */
const STATE_WORDS: Record<StageState, string> = {
  done: "done",
  current: "working",
  failed: "failed",
  todo: "to do",
  again: "ran before, to do again",
};

/** The station at the tip at of a stage's arm: the stage's name and state, who holds it and the harness and model of
 * its last turn, cut to room viewBox units (the station's accessible label has them whole), a rounds badge for a
 * step that ran in several rounds, and a verdict step's last verdict. */
function Station({ stage, record, waiting, at, room }: {
  stage: Stage; record: RunRecord; waiting: boolean; at: { x: number; y: number }; room: number;
}) {
  const radius = 27;
  const holder =
    stage.kind === "step" ? stage.roleLabel
    : stage.kind === "quality" ? "quality gate"
    : stage.kind === "publish" ? (record.pr ? prLabel(record.pr.url) : "pull request")
    : null;
  const last = stage.turns.at(-1);
  const harness = last ? harnessOf(last) : "";
  const rounds = stage.kind === "step" ? new Set(stage.turns.map((turn) => turn.round)).size : 0;
  const verdict = (record.verdicts ?? []).filter((v) => v.step === stage.step && stage.kind === "step").at(-1)?.verdict;
  const glyph =
    stage.state === "done" ? "✓"
    : stage.state === "failed" ? "✕"
    : stage.state === "current" ? (waiting ? "?" : "●")
    : "";
  const said = [
    stage.label, holder, harness, STATE_WORDS[stage.state], rounds > 1 && `${rounds} rounds`,
    verdict && `last verdict ${verdict}`, stage.loopTo && `loops to ${stage.loopTo}`,
  ];
  return (
    <g
      role="listitem"
      aria-label={said.filter(Boolean).join(", ")}
      className={`station ${stage.state}`}
      data-state={stage.state}
      transform={`translate(${at.x.toFixed(1)},${at.y.toFixed(1)})`}
    >
      {stage.state === "current" && <circle className="halo" r={radius + 4} />}
      <circle className="ring" r={radius} />
      <text className="label" y={glyph ? -2 : 4} textAnchor="middle">
        {stage.label}
      </text>
      {glyph && (
        <text className="glyph" y={13} textAnchor="middle">
          {glyph}
        </text>
      )}
      {holder && (
        <text className="holder" y={radius + 15} textAnchor="middle">
          {fit(holder, room, 11.5)}
        </text>
      )}
      {harness && (
        <text className="harness" y={radius + 28} textAnchor="middle">
          {fit(harness, room, 10)}
        </text>
      )}
      {rounds > 1 && (
        <g className="rounds">
          <circle cx={radius - 4} cy={-radius + 4} r={10} />
          <text x={radius - 4} y={-radius + 7.5} textAnchor="middle">
            ×{rounds}
          </text>
        </g>
      )}
      {verdict && (
        <g className={`verdict ${verdict === "APPROVE" ? "approve" : "changes"}`}>
          <rect x={-27} y={-radius - 20} width={54} height={15} rx={7.5} />
          <text y={-radius - 9} textAnchor="middle">
            {verdict === "APPROVE" ? "approve" : "changes"}
          </text>
        </g>
      )}
    </g>
  );
}

/** A turn's harness and model, as far as the record names them; "" for an analysis, which runs no agent. */
function harnessOf(turn: Turn): string {
  return [turn.agent, turn.model].filter(Boolean).join(" ");
}

/** Who ran a turn: its harness and model, or "quality gate" for an analysis. */
function turnHolder(turn: Turn): string {
  return turn.agent ? harnessOf(turn) : "quality gate";
}

/** A turn's step as the feed names it: with its quality round, as in "build q1", when it answers or is one. */
function turnStep(turn: Turn): string {
  return turn.quality_round ? `${turn.step} q${turn.quality_round}` : turn.step;
}

/** Whether something drives the run, so that a turn with no end is under way; in a stale run, or one nothing drives,
 * such a turn was cut short. */
function runDriven(record: RunRecord): boolean {
  return record.health === "running" && !record.error;
}

/** What the run does now, in a line: where it failed, how it ended, or its running turn and for how long. */
function nowLine(record: RunRecord, now: number): string {
  if (record.error) {
    return `failed at ${record.phase}`;
  }
  if (record.phase === "done") {
    return ["done", record.verdict, record.pr && "pull request open"].filter(Boolean).join(" · ");
  }
  const running = [...record.turns].reverse().find((turn) => turn.ended_at === null);
  if (!running) {
    return record.phase;
  }
  if (!runDriven(record)) {
    return `${turnStep(running)} · ${turnHolder(running)} · ${record.health ?? "stopped"}`;
  }
  const started = running.started_at ? Date.parse(running.started_at) : now;
  const doing = running.human_paced ? "waiting on you" : "running";
  return `${turnStep(running)} · ${turnHolder(running)} · ${doing} ${formatAge(Math.max(0, Math.round((now - started) / 1000)))}`;
}

/** How a turn in the feed stands: ended; failed, the turn the run failed in; running; or unfinished, a turn with no
 * end in a run nothing drives any more. */
type FeedState = "ended" | "failed" | "running" | "unfinished";

const FEED_GLYPHS: Record<FeedState, string> = { ended: "·", failed: "✕", running: "●", unfinished: "–" };

/** One row of the turn feed: the step with its quality round, the file, who ran it, and its length, or "running" or
 * "unfinished" for a turn with no end. */
function FeedRow({ turn, state }: { turn: Turn; state: FeedState }) {
  const length = state === "running" || state === "unfinished" ? state : turn.seconds === null ? "" : formatAge(turn.seconds);
  return (
    <li className={state === "ended" ? undefined : state}>
      <span>{FEED_GLYPHS[state]}</span>
      <span>{turnStep(turn)}</span>
      <span>
        {turn.file} · {turnHolder(turn)}
      </span>
      <span className="length">{length}</span>
    </li>
  );
}
