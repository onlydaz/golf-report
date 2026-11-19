import { describe, expect, it } from "vitest";
import { halfHourWindow, reportWindowBuckets } from "./reportBuckets";

describe("bounded report chart windows", () => {
  it("bounds a year of empty buckets without changing 30-minute granularity", () => {
    const from = "2026-01-01T00:00:00.000Z", to = "2027-01-01T00:00:00Z";
    const window = halfHourWindow([], from, to);
    expect(window.items.length).toBeLessThanOrEqual(reportWindowBuckets);
    expect(window.current).toBe(window.pages);
    const first = halfHourWindow([], from, to, 1);
    expect(first.items[0].at).toBe(from);
    expect(Date.parse(first.items[1].at) - Date.parse(first.items[0].at)).toBe(1800000);
    expect(Date.parse(window.items[window.items.length - 1].at) + 1800000).toBe(Date.parse(to));
  });
  it("keeps zero, OK and NG counts across window boundaries", () => {
    const from = "2026-01-01T00:00:00.000Z", boundary = "2026-01-08T00:00:00.000Z";
    const values = [{ at: from, ok: 0, ng: 2 }, { at: boundary, ok: 7, ng: 1 }];
    expect(halfHourWindow(values, from, "2026-01-09T00:00:00Z", 1).items[0]).toEqual(values[0]);
    expect(halfHourWindow(values, from, "2026-01-09T00:00:00Z", 2).items[0]).toEqual(values[1]);
    expect(halfHourWindow([], "invalid", boundary).items).toEqual([]);
    expect(halfHourWindow(undefined).items).toEqual([]);
  });
});

