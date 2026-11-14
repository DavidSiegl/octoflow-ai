import { describe, expect, it } from "vitest";
import { armOutline, armSlots, armTip, HEAD, spine } from "./runMap";

const stagesOf = (n: number) => armSlots(n).map((slot) => slot.stage);

describe("armSlots", () => {
  it("gives eight arms, the stages in the middle ones in order", () => {
    expect(stagesOf(3)).toEqual([null, null, 0, 1, 2, null, null, null]);
    expect(stagesOf(4)).toEqual([null, null, 0, 1, 2, 3, null, null]);
    expect(stagesOf(8)).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
    expect(stagesOf(0)).toEqual(Array(8).fill(null));
  });

  it("grows an arm for each stage past eight", () => {
    expect(stagesOf(10)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  });

  it("fans the arms below the head from left to right", () => {
    const tips = armSlots(5).map((slot) => armTip(slot.angle, 200));
    expect(tips.map((tip) => tip.x)).toEqual([...tips.map((tip) => tip.x)].sort((a, b) => a - b));
    expect(tips[0]?.x).toBeLessThan(HEAD.x);
    expect(tips.at(-1)?.x).toBeGreaterThan(HEAD.x);
    // The middle arms point down, the outer ones about level.
    expect(Math.min(...tips.map((tip) => tip.y))).toBeGreaterThan(HEAD.y);
  });
});

describe("spine", () => {
  it("keeps the tip where it is while the arm sways", () => {
    const angle = Math.PI / 3;
    const still = spine(angle, { reach: 200, curl: 0, sway: 0 });
    const swayed = spine(angle, { reach: 200, curl: 10, sway: 9 });
    expect(swayed[3]).toEqual(still[3]);
    expect(still[3]).toEqual(armTip(angle, 200));
    expect(swayed[1]).not.toEqual(still[1]);
  });

  it("outlines a closed path", () => {
    expect(armOutline(spine(Math.PI / 2, { reach: 200, curl: 0, sway: 0 }), 11, 3)).toMatch(/^M[\d.,L-]+Z$/);
  });
});
