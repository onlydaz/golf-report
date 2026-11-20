
export interface MeasurementChartSettings {
  readonly loadKg: number | null;
  readonly pushDistMm: number | null;
  readonly bendLowerMm: number | null;
  readonly bendUpperMm: number | null;
  readonly positionsMm: readonly (number | null)[];
}

export const measurementSettings: Readonly<Record<"BUTT" | "TIP", MeasurementChartSettings>> = {
  BUTT: { loadKg: 2.5, pushDistMm: 10, bendLowerMm: 50, bendUpperMm: 200, positionsMm: [150, 300, 450, 600] },
  TIP: { loadKg: 0.2, pushDistMm: 8, bendLowerMm: 50, bendUpperMm: 200, positionsMm: [80] },
};
