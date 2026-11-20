import { describe, expect, it } from "vitest";
import type { MeasurementDetail, MeasurementPoint } from "../services/measurementService";
import { buildButtAttempts } from "./reportAttempts";
import { bendExcess } from "./bendChart";
import { measurementSettings } from "../config/measurementSettings";

function point(number: number, values: Array<string | null>): MeasurementPoint {
  return {
    id: number, stationId: 1, pointNumber: number, result: "UNMEASURED",
    attemptCount: 0, decideAttempt: null, position: null,
    attempts: values.map((bend, index) => ({ id: number * 10 + index, pointId: number, attemptNumber: index + 1, pushDist: null, bend })),
  };
}
function measurement(id: number, measuredAt: string, points: MeasurementPoint[]): MeasurementDetail {
  return {
    id, measuredAt, createdAt: measuredAt, sourceId: 1, sessionId: "test", recordSeq: id,
    totalCount: id, productCode: "GOLD-X1", clubNumber: 1, overallResult: "OK",
    buttResult: "OK", tipResult: "OK", buttLoad: "2.5", tipLoad: "0.2",
    stations: [{ id: 1, measurementId: id, stationType: "BUTT", result: "OK", pointCount: 4,
      ngPoint: 0, load: "2.5", createdAt: measuredAt, points }],
  };
}

describe("report BUTT attempts", () => {
  it("retains retries, repeated bends and repeated club numbers with unique identity in chronological order", () => {
    const older = measurement(1, "2026-11-19T01:00:00Z", [point(2, ["110"]), point(1, ["100", "100"])]);
    const newer = measurement(2, "2026-11-19T02:00:00Z", [point(1, ["100"])]);
    const original = JSON.stringify([newer, older]);
    const samples = buildButtAttempts([newer, older]);
    expect(samples.map(sample => [sample.measurementId, sample.pointNumber, sample.attemptNumber, sample.bend]))
      .toEqual([[1, 1, 1, 100], [1, 1, 2, 100], [1, 2, 1, 110], [2, 1, 1, 100]]);
    expect(new Set(samples.map(sample => sample.key)).size).toBe(4);
    expect(JSON.stringify([newer, older])).toBe(original);
  });

  it("uses actual stored attempts, excluding empty/invalid bends and TIP without fabricating unused points", () => {
    const record = measurement(1, "2026-11-19T01:00:00Z", [point(1, [null, "", "NaN", "Infinity"]), point(2, []), point(3, ["0", "-1"]), point(5, ["100"])]);
    record.stations.push({ ...record.stations[0], id: 2, stationType: "TIP", points: [point(1, ["999"])] });
    expect(buildButtAttempts([record]).map(sample => [sample.pointNumber, sample.attemptNumber, sample.bend])).toEqual([[3, 1, 0], [3, 2, -1]]);
    expect(buildButtAttempts([])).toEqual([]);
  });

  it("orders equal timestamps by measurement ID and marks both sides of the limits without changing PLC results", () => {
    const records = [measurement(2, "2026-11-19T01:00:00Z", [point(1, ["201"])]),
      measurement(1, "2026-11-19T01:00:00Z", [point(1, ["49", "50", "200"])])];
    const samples = buildButtAttempts(records);
    expect(samples.map(sample => sample.measurementId)).toEqual([1, 1, 1, 2]);
    expect(samples.map(sample => bendExcess(sample.bend, measurementSettings.BUTT))).toEqual([1, 0, 0, 1]);
    expect(records.every(record => record.overallResult === "OK")).toBe(true);
  });
});
