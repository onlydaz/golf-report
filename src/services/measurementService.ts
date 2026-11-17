
import { invoke } from "@tauri-apps/api/core";

export type MeasurementResult = "UNMEASURED" | "OK" | "NG";

export type DatabaseConnectionState =
  | "CONNECTING"
  | "CONNECTED"
  | "CONNECTION_FAILED"
  | "CONNECTION_LOST"
  | "MIGRATION_FAILED";

export interface DatabaseStatus {
  state: DatabaseConnectionState;
  message: string;
}

export interface BackendMeasurement {
  id: number;
  sourceId: number;
  sessionId: string | null;
  recordSeq: number;
  totalCount: number | null;
  productCode: string;
  clubNumber: number;
  measuredAt: string;
  createdAt: string;
  tipLoad: string | null;
  buttLoad: string | null;
  overallResult: MeasurementResult;
  tipResult: MeasurementResult | null;
  buttResult: MeasurementResult | null;
}

export interface MeasurementAttempt {
  id: number;
  pointId: number;
  attemptNumber: number;
  pushDist: string | null;
  bend: string | null;
}

export interface MeasurementPoint {
  id: number;
  stationId: number;
  pointNumber: number;
  result: MeasurementResult;
  attemptCount: number;
  decideAttempt: number | null;
  position: string | null;
  attempts: MeasurementAttempt[];
}

export interface MeasurementStation {
  id: number;
  measurementId: number;
  stationType: "TIP" | "BUTT";
  result: MeasurementResult;
  pointCount: number;
  ngPoint: number | null;
  load: string;
  createdAt: string;
  points: MeasurementPoint[];
}

export interface MeasurementDetail extends BackendMeasurement {
  stations: MeasurementStation[];
}

export interface MeasurementListRequest {
  page?: number;
  pageSize?: number;
  search?: string;
  productCode?: string;
  clubNumber?: number;
  measuredFrom?: string;
  measuredTo?: string;
  tipResult?: MeasurementResult;
  buttResult?: MeasurementResult;
  result?: MeasurementResult;
}

export interface MeasurementListResponse {
  items: BackendMeasurement[];
  page: number;
  pageSize: number;
  total: number;
}

export interface MeasurementStatistics {
  totalMeasurements: number;
  totalOk: number;
  totalNg: number;
  passRate: number;
  tipNg: number;
  buttNg: number;

  daily: Array<{
    date: string;
    total: number;
    ok: number;
    ng: number;
  }>;

  stationResults: Array<{
    station: string;
    ok: number;
    ng: number;
  }>;

  overallDistribution: Array<{
    result: MeasurementResult;
    count: number;
  }>;
}

export const getDatabaseStatus = () =>
  invoke<DatabaseStatus>("get_database_status");

export const retryDatabaseConnection = () =>
  invoke<DatabaseStatus>("retry_database_connection");

export const getMeasurements = (request: MeasurementListRequest = {}) =>
  invoke<MeasurementListResponse>("get_measurements", { request });

export interface MeasurementSkuSummary {
  productCode: string;
  firstMeasuredAt: string;
  lastMeasuredAt: string;
  total: number;
  ok: number;
  ng: number;
  buttNg: number;
  tipNg: number;
}
export interface MeasurementSkuPage { items: MeasurementSkuSummary[]; total: number; page: number; pageSize: number }
export interface MeasurementHistoryItem extends BackendMeasurement {
  decisionPoints: Array<{ measurementId: number; station: "BUTT" | "TIP"; pointNumber: number; bend: string | null }>;
}
export interface MeasurementHistoryPage { items: MeasurementHistoryItem[]; total: number; page: number; pageSize: number }
export const getMeasurementSkus = (search = "", page = 1) =>
  invoke<{ items: string[]; hasMore: boolean }>("get_measurement_skus", { search, page });
export const getMeasurementSkuSummary = (request: MeasurementListRequest = {}) =>
  invoke<MeasurementSkuPage>("get_measurement_sku_summary", { request });
export const getMeasurementHistoryPage = (request: MeasurementListRequest = {}) =>
  invoke<MeasurementHistoryPage>("get_measurement_history_page", { request });

export const getMeasurementById = (id: number) =>
  invoke<MeasurementDetail>("get_measurement_by_id", { id });

export const getMeasurementStatistics = (
  request: MeasurementListRequest = {}
) =>
  invoke<MeasurementStatistics>("get_measurement_statistics", { request });

export interface PointQualityStatistics {
  productCode: string;
  station: "BUTT" | "TIP";
  pointNumber: number;
  position: string | null;
  measured: number;
  ng: number;
  retested: number;
  loadAvg: string | null;
  pushDistAvg: string | null;
  bendCount: number;
  bendMin: string | null;
  bendAvg: string | null;
  bendMax: string | null;
  bendStddev: string | null;
  histogram: Array<{ lower: string; upper: string; count: number }>;
}
export interface MeasurementQualityStatistics {
  halfHour?: Array<{ at: string; ok: number; ng: number }>;
  demoLimits?: { lower: string; upper: string } | null;
  demoHistogram?: Array<{ lower: string; upper: string; count: number; zone: "BELOW" | "WITHIN" | "ABOVE"; lowerInclusive: boolean; upperInclusive: boolean }>;
  buttSummary?: { count: number; mean: string | null; stddev: string | null } | null;
  retestedClubs: number;
  buttHistogram: Array<{ lower: string; upper: string; count: number }>;
  points: PointQualityStatistics[];
  pointSummary: PointSummaryStatistics[];
}
export interface PointSummaryStatistics {
  station: "BUTT" | "TIP";
  pointNumber: number;
  position: string | null;
  loadAvg: string | null;
  pushDistAvg: string | null;
  bendMin: string | null;
  bendAvg: string | null;
  bendMax: string | null;
  retested: number;
  ng: number;
}
export const getMeasurementQualityStatistics = (request: MeasurementListRequest = {}) =>
  invoke<MeasurementQualityStatistics>("get_measurement_quality_statistics", { request });

export const getMeasurementReport = (
  request: MeasurementListRequest = {}
) =>
  invoke<MeasurementDetail[]>("get_measurement_report", { request });
