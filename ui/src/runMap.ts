// The run map's geometry: where the octopus's arms go and the shape each one takes, in the SVG's own units. The
// component (components/RunMap.tsx) decides what each arm looks like; this module only where it lies.

/** A point in the map's viewBox, y pointing down. */
export interface Point {
  x: number;
  y: number;
}

/** The map's viewBox is 0 0 MAP_WIDTH MAP_HEIGHT. */
export const MAP_WIDTH = 640;
export const MAP_HEIGHT = 530;

/** The centre of the octopus's head; the arms grow from below it. */
export const HEAD: Point = { x: 320, y: 150 };

/** An octopus has eight arms; a run of more stages grows more. */
const MIN_ARMS = 8;

/** One arm of the octopus: its direction, in radians clockwise from pointing right (π/2 points straight down), and the
 * index of the stage it reaches to, or null for a spare arm, which stays curled. */
export interface ArmSlot {
  angle: number;
  stage: number | null;
}

/** The arms of a run of `stages` stages, left to right: eight, or one per stage past eight, fanned evenly below the
 * head from a little above level on the left to a little above level on the right. The stages take the middle arms
 * in order, the left one of the two middles when the spare arms do not split evenly. */
export function armSlots(stages: number): ArmSlot[] {
  const count = Math.max(MIN_ARMS, stages);
  const first = Math.floor((count - stages) / 2);
  const [from, to] = [Math.PI * 1.02, -Math.PI * 0.02];
  return Array.from({ length: count }, (_, i) => {
    const stage = i - first;
    return { angle: from + ((to - from) * i) / (count - 1), stage: stage >= 0 && stage < stages ? stage : null };
  });
}

/** How an arm lies, apart from its direction. */
export interface ArmPose {
  /** The distance, in viewBox units, from the head's centre to the tip, sideways; the tip sits a little higher. */
  reach: number;
  /** How far, in viewBox units, the arm's outer half bends back on itself; 0 for an arm held out. */
  curl: number;
  /** The sideways offset, in viewBox units, of the arm's middle at this moment; 0 at rest. Moves no tip. */
  sway: number;
}

/** The spine of the arm at `angle` in `pose`: a cubic Bézier's four points from its root under the head to its tip.
 * The tip depends on angle and reach alone, so a station drawn there stays put while the arm sways. */
export function spine(angle: number, pose: ArmPose): [Point, Point, Point, Point] {
  const { reach, curl, sway } = pose;
  const dir = { x: Math.cos(angle), y: Math.sin(angle) };
  const normal = { x: -dir.y, y: dir.x };
  const root = { x: HEAD.x + dir.x * 46, y: HEAD.y + 40 + dir.y * 30 };
  const tip = armTip(angle, reach);
  const c1 = {
    x: root.x + dir.x * reach * 0.35 + normal.x * (18 + sway),
    y: root.y + dir.y * reach * 0.35 + normal.y * (18 + sway),
  };
  const c2 = {
    x: tip.x - dir.x * reach * 0.25 - normal.x * (22 - sway + curl),
    y: tip.y - dir.y * reach * 0.25 - normal.y * (22 - sway + curl),
  };
  return [root, c1, c2, tip];
}

/** Where the tip of the arm at `angle` lies at `reach`: spine's last point. */
export function armTip(angle: number, reach: number): Point {
  return { x: HEAD.x + Math.cos(angle) * reach, y: HEAD.y + 40 + Math.sin(angle) * reach * 0.92 };
}

/** The point a fraction t (0 the root, 1 the tip) along the spine. */
export function along(points: readonly [Point, Point, Point, Point], t: number): Point {
  const [p0, p1, p2, p3] = points;
  const u = 1 - t;
  const [a, b, c, d] = [u * u * u, 3 * u * u * t, 3 * u * t * t, t * t * t];
  return { x: a * p0.x + b * p1.x + c * p2.x + d * p3.x, y: a * p0.y + b * p1.y + c * p2.y + d * p3.y };
}

/** An SVG path (its d attribute) outlining an arm along the spine, tapering from rootWidth to tipWidth: each the half
 * width, in viewBox units. */
export function armOutline(points: readonly [Point, Point, Point, Point], rootWidth: number, tipWidth: number): string {
  const segments = 28;
  const left: string[] = [];
  const right: string[] = [];
  for (let i = 0; i <= segments; i++) {
    const t = i / segments;
    const p = along(points, t);
    const ahead = along(points, Math.min(1, t + 0.01));
    const behind = along(points, Math.max(0, t - 0.01));
    const length = Math.hypot(ahead.x - behind.x, ahead.y - behind.y) || 1;
    const [dx, dy] = [(ahead.x - behind.x) / length, (ahead.y - behind.y) / length];
    const w = rootWidth + (tipWidth - rootWidth) * t;
    left.push(`${(p.x - dy * w).toFixed(1)},${(p.y + dx * w).toFixed(1)}`);
    right.push(`${(p.x + dy * w).toFixed(1)},${(p.y - dx * w).toFixed(1)}`);
  }
  return `M${left.join("L")}L${right.reverse().join("L")}Z`;
}
