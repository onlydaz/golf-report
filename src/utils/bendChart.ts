import type { MeasurementChartSettings } from "../config/measurementSettings";

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
