import { describe, expect, it } from "vitest";
import { measurementSettings } from "../config/measurementSettings";
import { bendAxisTicks, bendDataDomain, bendDomain, bendExcess, bendIntervalZone } from "./bendChart";

describe("fixed chart setpoints", () => {
  it("fits the monitor to real values with small padding instead of expanding to limits", () => {
    expect(bendDataDomain([100, 101, 104, null, NaN, Infinity])).toEqual([95, 109]);
    expect(bendDataDomain([100, 300])).toEqual([90, 310]);
    expect(bendDataDomain([0, 0])).toEqual([-5, 5]);
    expect(bendDataDomain([-3.5])).toEqual([-8.5, 1.5]);
    expect(bendDataDomain([null, NaN])).toEqual([0, 1]);
    const ticks = bendAxisTicks(bendDataDomain([100, 104]), measurementSettings.BUTT);
    expect(ticks).not.toContain(50);
    expect(ticks).not.toContain(200);
    expect(ticks.every(value => value >= 95 && value <= 109)).toBe(true);
  });
  it("keeps a nearby limit visible while retaining outlier classification", () => {
    for (const settings of [measurementSettings.BUTT, measurementSettings.TIP]) {
      const domain = bendDataDomain([49, 51]);
      expect(bendAxisTicks(domain, settings)).toContain(50);
      expect(bendAxisTicks(domain, settings)).not.toContain(200);
      expect(bendExcess(49, settings)).toBe(1);
      expect(bendExcess(51, settings)).toBe(0);
      expect(bendExcess(201, settings)).toBe(1);
    }
  });
  it.each(["BUTT", "TIP"] as const)("always includes both limit ticks for %s across different measurement ranges", station => {
    const settings = measurementSettings[station];
    for (const values of [[0, 2], [80, 120], [-500, 5000], [null], []]) {
      const domain = bendDomain(values, settings);
      const ticks = bendAxisTicks(domain, settings);
      expect(ticks).toContain(50);
      expect(ticks).toContain(200);
      expect(ticks).toEqual([...new Set(ticks)].sort((a, b) => a - b));
      expect(ticks.every(value => value >= domain[0] && value <= domain[1])).toBe(true);
    }
  });
  it("uses shared 50–200 mm limits for both stations and uses kilogram setpoints", () => {
    expect(measurementSettings.BUTT).toEqual({ loadKg: 2.5, pushDistMm: 10, bendLowerMm: 50, bendUpperMm: 200, positionsMm: [150, 300, 450, 600] });
    expect(measurementSettings.TIP).toEqual({ loadKg: 0.2, pushDistMm: 8, bendLowerMm: 50, bendUpperMm: 200, positionsMm: [80] });
  });
  it.each(["BUTT", "TIP"] as const)("classifies report bins conservatively using the same %s limits", station => {
    const settings = measurementSettings[station];
    expect(bendIntervalZone(0, 50, false, settings)).toBe("BELOW");
    expect(bendIntervalZone(0, 50, true, settings)).toBe("MIXED");
    expect(bendIntervalZone(50, 200, true, settings)).toBe("WITHIN");
    expect(bendIntervalZone(190, 210, true, settings)).toBe("MIXED");
    expect(bendIntervalZone(201, 250, true, settings)).toBe("ABOVE");
    expect(bendIntervalZone(NaN, 200, true, settings)).toBe("UNKNOWN");
    const [lo, hi] = bendDomain([0, 2], settings);
    expect(lo).toBeLessThan(0); expect(hi).toBeGreaterThan(200);
  });
  it("compares finite values to limits without changing PLC result data", () => {
    const settings = measurementSettings.BUTT;
    for (const value of [50, 125, 200]) expect(bendExcess(value, settings)).toBe(0);
    expect(bendExcess(49, settings)).toBe(1);
    expect(bendExcess(201, settings)).toBe(1);
    expect(bendExcess(NaN, settings)).toBeNull();
    expect(bendExcess(100)).toBeNull();
    expect(bendExcess(-10, { ...settings, bendLowerMm: null })).toBe(0);
    expect(bendExcess(100, { ...settings, bendUpperMm: null })).toBe(0);
  });
  it("includes limits in the Y domain, preserving zero/negative values and ignoring NULL", () => {
    const [lo, hi] = bendDomain([0, 0, null, -0.5], { ...measurementSettings.TIP, bendLowerMm: -50, bendUpperMm: 100 });
    expect(lo).toBeLessThan(-50); expect(hi).toBeGreaterThan(100);
    expect(bendDomain([])).toEqual([0, 1]);
    const zero = bendDomain([0]);
    expect(zero[0]).toBeLessThan(0); expect(zero[1]).toBeGreaterThan(0);
  });
});
