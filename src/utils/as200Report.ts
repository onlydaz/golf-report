import { formatReportTime } from "./reportTime";
import type { MeasurementDetail, MeasurementPoint } from "../services/measurementService";

// Old records may contain padding position=0. Only a wholly unused point is empty;
// do not hide conflicting attempts or a nonzero position behind a display placeholder.
export function isUnmeasuredPoint(point: MeasurementPoint): boolean {
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
    "TIP Load (g)": item.tipLoad ?? "",
    "BUTT Result": item.buttResult ?? "",
    "BUTT Load (g)": item.buttLoad ?? "",
  }));

  const points = records.flatMap(item =>
    item.stations.flatMap(station =>
      station.points.map(point => ({
        "Measurement ID": item.id,
        StationType: station.stationType,
        PointNumber: point.pointNumber,
        "Position (mm)": isUnmeasuredPoint(point) ? "" : point.position ?? "",
        Result: point.result,
        AttemptCount: point.attemptCount,
        DecideAttempt: point.decideAttempt ?? "",
      }))
    )
  );

  const attempts = records.flatMap(item =>
    item.stations.flatMap(station =>
      station.points.flatMap(point =>
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
