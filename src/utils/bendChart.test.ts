import { describe, expect, it } from "vitest";
import { measurementSettings } from "../config/measurementSettings";
import { bendDomain, bendExcess } from "./bendChart";

describe("fixed chart setpoints", () => {
  it("retains the QC-confirmation defaults from the former DEMO for each station", () => {
    expect(measurementSettings.BUTT).toEqual({ loadG: 500, pushDistMm: 10, bendLowerMm: 0, bendUpperMm: 2, positionsMm: [150, 300, 450, 600] });
    expect(measurementSettings.TIP).toEqual({ loadG: 200, pushDistMm: 8, bendLowerMm: 0, bendUpperMm: 2, positionsMm: [80] });
  });
  it("compares finite values to limits without changing PLC result data", () => {
    const settings = measurementSettings.BUTT;
    for (const value of [0, 1, 2]) expect(bendExcess(value, settings)).toBe(0);
    expect(bendExcess(-1, settings)).toBe(1);
    expect(bendExcess(3, settings)).toBe(1);
    expect(bendExcess(NaN, settings)).toBeNull();
    expect(bendExcess(100)).toBeNull();
    expect(bendExcess(-10, { ...settings, bendLowerMm: null })).toBe(0);
    expect(bendExcess(10, { ...settings, bendUpperMm: null })).toBe(0);
  });
  it("includes limits in the Y domain, preserving zero/negative values and ignoring NULL", () => {
    const [lo, hi] = bendDomain([0, 0, null, -0.5], { ...measurementSettings.TIP, bendLowerMm: -50, bendUpperMm: 100 });
    expect(lo).toBeLessThan(-50); expect(hi).toBeGreaterThan(100);
    expect(bendDomain([])).toEqual([0, 1]);
    const zero = bendDomain([0]);
    expect(zero[0]).toBeLessThan(0); expect(zero[1]).toBeGreaterThan(0);
  });
});
