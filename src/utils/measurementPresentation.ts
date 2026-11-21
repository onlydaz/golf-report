import { measurementSettings } from "../config/measurementSettings";

export type MeasurementScalar = string | number | null | undefined;
export type MeasurementStationType = "BUTT" | "TIP";
export interface PresentedMeasurementValue {
  value: string | null;
  raw: string | null;
  source: "PLC" | "DEFAULT" | "MIXED" | "MISSING" | "INVALID";
  samples?: number;
  defaultSamples?: number;
}

const decimal = /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/;

/** Presentation only. Never mutate a DTO, stored NUMERIC, or the raw WORDs. */
function realValue(value: unknown, fallback: number | null): PresentedMeasurementValue {
  if (value != null && typeof value !== "string" && typeof value !== "number") {
    return { value: null, raw: `kiểu ${Array.isArray(value) ? "array" : typeof value}`, source: "INVALID" };
  }
  const raw = value == null ? null : String(value).trim();
  if (raw == null || raw === "") return { value: null, raw, source: "MISSING" };
  if (!decimal.test(raw) || !Number.isFinite(Number(raw))) {
    return { value: null, raw, source: "INVALID" };
  }
  // Test the decimal mantissa, so a nonzero value that underflows JS Number
  // cannot accidentally become a default. -0 and 0.000 are exact zero too.
  const zero = !/[1-9]/.test(raw.split(/[eE]/)[0]);
  if (zero && fallback != null && Number.isFinite(fallback)) {
    return { value: String(fallback), raw, source: "DEFAULT" };
  }
  return { value: raw, raw, source: "PLC" };
}

export const presentLoad = (value: MeasurementScalar, station: MeasurementStationType) =>
  realValue(value, measurementSettings[station].loadKg);

export const presentPosition = (value: MeasurementScalar, station: MeasurementStationType, pointNumber: number) =>
  realValue(value, Number.isInteger(pointNumber) && pointNumber > 0
    ? measurementSettings[station].positionsMm[pointNumber - 1] ?? null : null);

/** SQL retains its raw average and reports counts from the SAME sample set.
 * Apply the default per zero sample, never to the resulting average itself.
 * No change to Bend, result classification, filtering or sample membership.
 */
export function presentLoadAverage(value: MeasurementScalar, station: MeasurementStationType,
  samples: number | undefined, zeroSamples: number | undefined): PresentedMeasurementValue {
  const original = realValue(value, null);
  if (original.source !== "PLC") return original;
  if (samples == null || zeroSamples == null || !Number.isSafeInteger(samples)
    || !Number.isSafeInteger(zeroSamples) || samples <= 0 || zeroSamples < 0 || zeroSamples > samples) {
    return { ...original, value: null, source: "INVALID" };
  }
  if (zeroSamples === 0) return original;
  const fallback = measurementSettings[station].loadKg;
  if (fallback == null || !Number.isFinite(fallback)) return { ...original, value: null, source: "INVALID" };
  const adjusted = Number(original.value) + fallback * zeroSamples / samples;
  if (!Number.isFinite(adjusted)) return { ...original, value: null, source: "INVALID" };
  return { value: String(adjusted), raw: original.raw, samples, defaultSamples: zeroSamples,
    source: zeroSamples === samples ? "DEFAULT" : "MIXED" };
}

export function measurementValueDescription(value: PresentedMeasurementValue): string {
  if (value.source === "INVALID") return `LỖI: dữ liệu hoặc metadata thống kê không hợp lệ (${value.raw ?? "—"}); không dùng mặc định.`;
  if (value.source === "MISSING") return "Chưa có giá trị hợp lệ; không dùng mặc định cho NULL hoặc dữ liệu thiếu.";
  if (value.defaultSamples != null) return `${value.source}: trung bình hiển thị dùng mặc định cho ${value.defaultSamples}/${value.samples} mẫu Load=0; trung bình gốc ${value.raw}.`;
  return value.source === "DEFAULT"
    ? `DEFAULT: giá trị PLC lưu gốc là ${value.raw}; hiển thị mặc định ${value.value}. Dữ liệu gốc giữ nguyên.`
    : `PLC: dùng giá trị lưu gốc ${value.raw}.`;
}

/** Keep malformed text visible in exports instead of replacing it with a default. */
export const exportMeasurementValue = (value: PresentedMeasurementValue): string =>
  value.source === "INVALID" ? value.raw ?? "" : value.value ?? "";
