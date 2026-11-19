/**
 * DEFAULT CHART SETPOINTS — QC MUST CONFIRM BEFORE PRODUCTION USE.
 * Load/push/positions retain the former DEMO defaults; bend limits are 50–200 mm.
 * These setpoints are NOT from PLC registers. loadG remains in grams internally.
 * Technicians edit this file and rebuild the application to change them.
 * They apply to every SKU, by station, and never determine PLC OK/NG or KPI.
 * TIP position 80 mm is retained from simulatorDemoSettings in Git HEAD.
 * Use null for a limit/setting that QC has not specified; never infer it from measurements.
 */
export interface MeasurementChartSettings {
  readonly loadG: number | null;
  readonly pushDistMm: number | null;
  readonly bendLowerMm: number | null;
  readonly bendUpperMm: number | null;
  readonly positionsMm: readonly (number | null)[];
}

export const measurementSettings: Readonly<Record<"BUTT" | "TIP", MeasurementChartSettings>> = {
  BUTT: { loadG: 500, pushDistMm: 10, bendLowerMm: 50, bendUpperMm: 200, positionsMm: [150, 300, 450, 600] },
  TIP: { loadG: 200, pushDistMm: 8, bendLowerMm: 50, bendUpperMm: 200, positionsMm: [80] },
};
