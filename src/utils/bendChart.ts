import type { MeasurementChartSettings } from "../config/measurementSettings";

/** Aggregated bins crossing a limit cannot identify which individual samples exceed it. */
export function bendIntervalZone(lower: number, upper: number, upperInclusive: boolean, settings: MeasurementChartSettings) {
  const lo = settings.bendLowerMm, hi = settings.bendUpperMm;
  if (![lower, upper].every(Number.isFinite) || lower > upper || (lo == null && hi == null)) return "UNKNOWN";
  if (lo != null && (upper < lo || (upper === lo && !upperInclusive))) return "BELOW";
  if (hi != null && lower > hi) return "ABOVE";
  if ((lo == null || lower >= lo) && (hi == null || upper <= hi)) return "WITHIN";
  return "MIXED";
}

/** Visual comparison only; never infer or update the PLC result. */
export function bendExcess(value: number, settings?: MeasurementChartSettings | null): number | null {
  if (!Number.isFinite(value) || (settings?.bendLowerMm == null && settings?.bendUpperMm == null)) return null;
  return Math.max(0, settings?.bendLowerMm == null ? 0 : settings.bendLowerMm - value,
    settings?.bendUpperMm == null ? 0 : value - settings.bendUpperMm);
}

export function bendDomain(values: Array<number | null>, settings?: MeasurementChartSettings | null): [number, number] {
  const finite = [...values, settings?.bendLowerMm, settings?.bendUpperMm]
    .filter((v): v is number => v != null && Number.isFinite(v));
  if (!finite.length) return [0, 1];
  const lo = Math.min(...finite), hi = Math.max(...finite);
  const padding = Math.max((hi - lo) * 0.1, Math.max(Math.abs(lo), Math.abs(hi), 1) * 0.02);
  return [lo - padding, hi + padding];
}

/** Fit measured values only, with 5–10 mm of padding; limits must not stretch this view. */
export function bendDataDomain(values: Array<number | null>): [number, number] {
  const finite = values.filter((value): value is number => value != null && Number.isFinite(value));
  if (!finite.length) return [0, 1];
  const lo = Math.min(...finite), hi = Math.max(...finite);
  const padding = Math.min(10, Math.max(5, (hi - lo) * 0.1));
  return [lo - padding, hi + padding];
}

/** Keep visible limits as explicit ticks without pulling off-screen limits into the domain. */
export function bendAxisTicks(domain: [number, number], settings: MeasurementChartSettings): number[] {
  const [lo, hi] = domain;
  const limits = [settings.bendLowerMm, settings.bendUpperMm]
    .filter((value): value is number => value != null && Number.isFinite(value) && value >= lo && value <= hi);
  const step = (hi - lo) / 4;
  const regular = Array.from({ length: 5 }, (_, index) => Number((lo + index * step).toPrecision(6)))
    .filter(value => value >= lo && value <= hi && limits.every(limit => Math.abs(value - limit) > step * 0.35));
  return [...new Set([...regular, ...limits])].sort((a, b) => a - b);
}
