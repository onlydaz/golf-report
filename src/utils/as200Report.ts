import { formatReportTime } from "./reportTime";
import type { MeasurementDetail, MeasurementPoint, MeasurementStation } from "../services/measurementService";

/** Presentation only: keep the stored PLC club number unchanged. */
export function formatClubNumber(value: number | string | null | undefined): string {
  if (value == null) return "—";
  const number = String(value).trim().replace(/^#+\s*/, "");
  return number ? `#${number}` : "—";
}

export function stationPointNumbers(station: "TIP" | "BUTT"): readonly number[] {
  return station === "TIP" ? [1] : [1, 2, 3, 4];
}

export function isBusinessPoint(station: string, pointNumber: number): boolean {
  return Number.isInteger(pointNumber) && pointNumber >= 1
    && (station === "TIP" ? pointNumber === 1 : station === "BUTT" && pointNumber <= 4);
}

export function displayPointResult(_station: Pick<MeasurementStation, "result" | "pointCount">, point: MeasurementPoint) {
  return point.result; // Preserve the PLC value; do not infer SKIPPED from station metadata.
}

// Old records may contain padding position=0. Only a wholly unused point is empty;
// do not hide conflicting attempts or a nonzero position behind a display placeholder.
export function isUnmeasuredPoint(point: Pick<MeasurementPoint, "attemptCount" | "decideAttempt" | "attempts" | "position"> & { result: string }): boolean {
  return ["UNMEASURED", "SKIPPED"].includes(String(point.result))
    && point.attemptCount === 0 && point.decideAttempt == null && point.attempts.length === 0
    && (point.position == null || Number(point.position) === 0);
}

/** Format for display only; raw decimal strings stay unchanged in exports and storage. */
export function formatDecimal(value: string | number | null | undefined): string {
  if (value == null || (typeof value === "string" && !value.trim())) return "—";
  const number = Number(value);
  return Number.isFinite(number)
    ? number.toLocaleString("vi-VN", { minimumFractionDigits: 2, maximumFractionDigits: 2 })
    : "—";
}

// PLC and PostgreSQL Load values are already kilograms.
export const formatLoadKg = formatDecimal;

export function buildReportTables(records: MeasurementDetail[]) {
  const measurements = records.map(item => ({
    "Measurement ID": item.id,
    ProductCode: item.productCode,
    StickNo: item.clubNumber,
    MeasuredAt: formatReportTime(item.measuredAt),
    RecordSeq: item.recordSeq,
    TotalCount: item.totalCount ?? "",
    OverallResult: item.overallResult,
    "TIP Result": item.tipResult ?? "",
    "TIP Load (kg)": item.tipLoad ?? "",
    "BUTT Result": item.buttResult ?? "",
    "BUTT Load (kg)": item.buttLoad ?? "",
  }));

  const points = records.flatMap(item =>
    item.stations.flatMap(station =>
      station.points.filter(point => isBusinessPoint(station.stationType, point.pointNumber)).map(point => ({
        "Measurement ID": item.id,
        StationType: station.stationType,
        PointNumber: point.pointNumber,
        "Position (mm)": isUnmeasuredPoint(point) ? "" : point.position ?? "",
        Result: displayPointResult(station, point),
        AttemptCount: point.attemptCount,
        DecideAttempt: point.decideAttempt ?? "",
      }))
    )
  );

  const attempts = records.flatMap(item =>
    item.stations.flatMap(station =>
      station.points.filter(point => isBusinessPoint(station.stationType, point.pointNumber)).flatMap(point =>
        point.attempts.map(attempt => ({
          "Measurement ID": item.id,
          StationType: station.stationType,
          PointNumber: point.pointNumber,
          AttemptNumber: attempt.attemptNumber,
          "PushDist (mm)": attempt.pushDist ?? "",
          "Bend (mm)": attempt.bend ?? "",
        }))
      )
    )
  );

  return {
    measurements,
    points,
    attempts,
  };
}
