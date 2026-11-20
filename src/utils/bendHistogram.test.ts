import { describe, expect, it } from "vitest";
import { buildBendHistogram } from "./bendHistogram";

describe("BUTT attempt histogram", () => {
  it("counts each attempt once with inclusive specification limits and half-open interior bins", () => {
    const result = buildBendHistogram([49, 50, 59.9, 60, 100, 100, 190, 200, 200.1], 50, 200)!;
    expect(result.bins).toHaveLength(17);
    expect(result.bins[0].count).toBe(1);
    expect(result.bins[1].count).toBe(2);
    expect(result.bins[2].count).toBe(1);
    expect(result.bins[6].count).toBe(2);
    expect(result.bins[15].count).toBe(2);
    expect(result.bins[16].count).toBe(1);
    expect(result.total).toBe(9);
    expect(result.within).toBe(7);
    expect(result.outside).toBe(2);
    expect(result.passRate).toBeCloseTo(100 * 7 / 9);
    expect(result.bins.reduce((sum, bin) => sum + bin.count, 0)).toBe(9);
    expect(result.bins.reduce((sum, bin) => sum + bin.percent, 0)).toBeCloseTo(100);
  });
  it("keeps the same number of bins for 5000 identical attempts without losing repeats", () => {
    const result = buildBendHistogram(Array(5000).fill(110), 50, 200)!;
    expect(result.total).toBe(5000);
    expect(result.bins).toHaveLength(17);
    expect(result.bins[7].count).toBe(5000);
    expect(result.passRate).toBe(100);
  });
  it("preserves empty bins, ignores invalid numbers, and handles changed limits and partial last bins", () => {
    const empty = buildBendHistogram([NaN, Infinity], 50, 200)!;
    expect(empty.total).toBe(0);
    expect(empty.passRate).toBeNull();
    expect(empty.bins.every(bin => bin.count === 0 && bin.percent === 0)).toBe(true);
    const changed = buildBendHistogram([-6, -5, 5, 15, 20, 21], -5, 20)!;
    expect(changed.bins.map(bin => bin.count)).toEqual([1, 1, 1, 2, 1]);
    expect(changed.bins[3]).toMatchObject({ lower: 15, upper: 20 });
    expect(buildBendHistogram([], null, 200)).toBeNull();
    expect(buildBendHistogram([], 200, 50)).toBeNull();
  });
});
