/**
 * DEFAULT CHART SETPOINTS — QC MUST CONFIRM BEFORE PRODUCTION USE.
 * These values come from the former DEMO source, NOT from PLC registers.
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
  BUTT: { loadG: 500, pushDistMm: 10, bendLowerMm: 0, bendUpperMm: 2, positionsMm: [150, 300, 450, 600] },
  TIP: { loadG: 200, pushDistMm: 8, bendLowerMm: 0, bendUpperMm: 2, positionsMm: [80] },
};
