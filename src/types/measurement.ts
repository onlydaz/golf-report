export type MeasurementStatus = "OK" | "NG";
export type TipPositionResult = "OK" | "NG" | "SKIPPED" | "INCOMPLETE";
export type MeasurementResult = "OK" | "NG" | "INCOMPLETE";

export interface TipPositionMeasurement {
  positionNumber: number;
  finalDistance: number | null;
  retryCount: number | null;
  result: TipPositionResult;
  // Frontend-only attempt awaiting a result; excluded from the save payload.
  pendingRetryCount?: number;
}

export interface ButtMeasurement {
  finalDistance: number | null;
  result: MeasurementStatus | null;
}

export interface MeasurementInput {
  productCode: string;
  measurementTime: string;
  clubNumber: string;
  weight: number | null;
  tips: TipPositionMeasurement[];
  butt: ButtMeasurement;
  tipResult: MeasurementResult;
  overallResult: MeasurementResult;
}

export interface TipMeasurement {
  position: string;
  angle: number;
  diameter: number;
  runout: number;
  status: MeasurementStatus;
}

export interface Measurement {
  id: string;
  measuredAt: string;
  productCode: string;
  productName: string;
  operator: string;
  station: string;
  status: MeasurementStatus;
  buttDiameter: number;
  buttRoundness: number;
  tips: TipMeasurement[];
}
