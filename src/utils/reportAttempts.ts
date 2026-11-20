import type { MeasurementDetail } from "../services/measurementService";

/** Keep every persisted finite BUTT attempt, including retries and equal values.
 * Attempts have no timestamp: within a measurement use point/attempt order.
 */
export function buildButtAttempts(measurements: MeasurementDetail[]) {
  return [...measurements]
    .sort((a, b) => Date.parse(a.measuredAt) - Date.parse(b.measuredAt) || a.id - b.id)
    .flatMap(measurement => measurement.stations
      .filter(station => station.stationType === "BUTT")
      .flatMap(station => [...station.points]
        .filter(point => point.pointNumber >= 1 && point.pointNumber <= 4)
        .sort((a, b) => a.pointNumber - b.pointNumber || a.id - b.id)
        .flatMap(point => [...point.attempts]
          .sort((a, b) => a.attemptNumber - b.attemptNumber || a.id - b.id)
          .flatMap(attempt => {
            if (attempt.bend == null || attempt.bend.trim() === "") return [];
            const bend = Number(attempt.bend);
            if (!Number.isFinite(bend)) return [];
            return [{
              key: `${measurement.id}-${point.id}-${attempt.id}`,
              measurementId: measurement.id,
              productCode: measurement.productCode,
              clubNumber: measurement.clubNumber,
              measuredAt: measurement.measuredAt,
              pointNumber: point.pointNumber,
              attemptNumber: attempt.attemptNumber,
              bend,
            }];
          }))));
}
