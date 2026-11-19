import { formatReportTime } from "./reportTime";
import type { MeasurementDetail, MeasurementPoint, MeasurementStation } from "../services/measurementService";

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

export function formatDecimal(value: string | null | undefined): string {
  if (value == null) return "—";

  const scientific = /^(-?)(\d+)(?:\.(\d+))?[eE]([+-]?\d+)$/.exec(value);

  if (scientific) {
    const [, sign, whole, fraction = "", exponent] = scientific;
    const digits = whole + fraction;
    const decimalAt = whole.length + Number(exponent);

    value =
      sign +
      (decimalAt <= 0
        ? `0.${"0".repeat(-decimalAt)}${digits}`
        : decimalAt >= digits.length
          ? digits + "0".repeat(decimalAt - digits.length)
          : `${digits.slice(0, decimalAt)}.${digits.slice(decimalAt)}`);
  }

  return value.replace(
    /^(-?\d+)(?:\.(\d+))?$/,
    (_, integer: string, fraction?: string) =>
      integer.replace(/\B(?=(\d{3})+(?!\d))/g, ".") +
      (fraction ? `,${fraction}` : "")
  );
}

/** Convert stored grams at the presentation boundary, preserving decimal precision. */
export function gramsToKilograms(value: string | number | null | undefined): string | null {
  if (value == null) return null;
  const match = /^([+-]?)(\d+)(?:\.(\d*))?(?:[eE]([+-]?\d+))?$/.exec(String(value));
  if (!match) return null;
  const [, sign, whole, fraction = "", exponent = "0"] = match;
  const digits = whole + fraction;
  const decimalAt = whole.length + Number(exponent) - 3;
  if (!Number.isSafeInteger(decimalAt) || Math.abs(decimalAt) > 10000) return null;
  const shifted = decimalAt <= 0 ? `0.${"0".repeat(-decimalAt)}${digits}`
    : decimalAt >= digits.length ? digits + "0".repeat(decimalAt - digits.length)
      : `${digits.slice(0, decimalAt)}.${digits.slice(decimalAt)}`;
  const normalized = shifted.replace(/^0+(?=\d)/, "").replace(/(\.\d*?)0+$/, "$1").replace(/\.$/, "");
  return `${sign === "-" && normalized !== "0" ? "-" : ""}${normalized}`;
}

export const formatLoadKg = (value: string | number | null | undefined): string => formatDecimal(gramsToKilograms(value));

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
    "TIP Load (kg)": gramsToKilograms(item.tipLoad) ?? "",
    "BUTT Result": item.buttResult ?? "",
    "BUTT Load (kg)": gramsToKilograms(item.buttLoad) ?? "",
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
