import { describe, expect, it } from "vitest";
import type { MeasurementHistoryItem } from "../../services/measurementService";
import { buildBendSeries, decisionBend } from "./MonitorPanels";

function record(id: number, productCode = "SKU-A"): MeasurementHistoryItem {
  return {
    id, productCode, sourceId: 1, sessionId: "test", recordSeq: id, totalCount: id,
    clubNumber: 1, measuredAt: new Date(Date.UTC(2026, 10, 16, 0, id)).toISOString(),
    createdAt: "2026-11-16T00:00:00Z", overallResult: id % 2 ? "NG" : "OK",
    buttResult: id % 2 ? "NG" : "OK", tipResult: "OK", buttLoad: null, tipLoad: null,
    decisionPoints: [
      { measurementId: id, station: "BUTT", pointNumber: 1, bend: "-0.5" },
      { measurementId: id, station: "BUTT", pointNumber: 2, bend: "0" },
      { measurementId: id, station: "BUTT", pointNumber: 3, bend: "1.25" },
      { measurementId: id, station: "BUTT", pointNumber: 4, bend: null },
      { measurementId: id, station: "TIP", pointNumber: 1, bend: "9.5" },
    ],
  };
}

describe("stored monitor decision bends", () => {
  it.each([0, 1, 10, 20, 30, 60])("plots up to 30 newest measurements out of %i without requiring setpoints", count => {
    const records = Array.from({ length: count }, (_, index) => record(index + 1)).reverse();
    const series = buildBendSeries(records, "SKU-A", "BUTT");
    expect(series).toHaveLength(Math.min(count, 30));
    expect(series.map(row => row.id)).toEqual(Array.from({ length: Math.min(count, 30) }, (_, i) => Math.max(1, count - 29) + i));
    for (const row of series) {
      expect([row.p1, row.p2, row.p3, row.p4]).toEqual([-0.5, 0, 1.25, null]);
    }
  });

  it("separates stations and SKU; preserves repeated StickNo and RecordSeq using measurement IDs", () => {
    const first = record(1), second = { ...record(2), recordSeq: 1, sessionId: "another-session" };
    const records = [record(60, "OTHER"), first, second, first];
    expect(buildBendSeries(records, "SKU-A", "TIP").map(row => [row.id, row.p1, row.p2])).toEqual([[1, 9.5, null], [2, 9.5, null]]);
    expect(buildBendSeries(records, undefined, "BUTT").map(row => row.id)).toEqual([1, 2, 60]);
    expect(buildBendSeries(records, "MISSING", "BUTT")).toEqual([]);
    expect(records).toHaveLength(4);
  });

  it("keeps NULL/unmeasured values empty and zero valid, without borrowing another row's points", () => {
    const a = record(1), b = record(2);
    b.decisionPoints = [{ measurementId: 2, station: "BUTT", pointNumber: 4, bend: "-2" }];
    expect(decisionBend(a, "BUTT", 2)).toBe("0");
    expect(decisionBend(a, "BUTT", 4)).toBeNull();
    expect(decisionBend(b, "BUTT", 1)).toBeNull();
    expect(decisionBend(b, "BUTT", 4)).toBe("-2");
    expect(buildBendSeries([a, b], "SKU-A", "BUTT")[1].p4).toBe(-2);
    expect(a.overallResult).toBe("NG");
    expect(b.overallResult).toBe("OK");
  });

  it("refresh/add uses the newest 30 in time order and breaks equal timestamps by database ID", () => {
    const records = Array.from({ length: 30 }, (_, i) => record(i + 1));
    const old = buildBendSeries(records, "SKU-A", "BUTT");
    const next = buildBendSeries([record(31), ...records], "SKU-A", "BUTT");
    expect(old[0].id).toBe(1);
    expect(next[0].id).toBe(2);
    expect(next[next.length - 1]?.id).toBe(31);
    expect(new Set(next.map(row => row.id)).size).toBe(30);
    const sameTime = { ...record(100), measuredAt: records[0].measuredAt };
    expect(buildBendSeries([sameTime, records[0]], "SKU-A", "BUTT").map(row => row.id)).toEqual([1, 100]);
  });
  it("shows the newest 30 across PLC ProductCodes including N/A when no SKU filter is supplied", () => {
    const records = Array.from({ length: 35 }, (_, index) => record(index + 1, ["X212PI", "GOLD-X2", "N/A"][index % 3]));
    const series = buildBendSeries(records, undefined, "BUTT");
    expect(series.map(row => row.id)).toEqual(Array.from({ length: 30 }, (_, index) => index + 6));
    expect(series.every(row => row.p1 === -0.5)).toBe(true);
    expect(records.some(row => row.productCode === "N/A")).toBe(true);
  });
});
