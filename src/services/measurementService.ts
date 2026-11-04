import { invoke } from "@tauri-apps/api/core";
import type { MeasurementInput, TipPositionMeasurement } from "../types/measurement";

export type DatabaseConnectionState = "CONNECTING" | "CONNECTED" | "CONNECTION_FAILED" | "MIGRATION_FAILED";

export interface DatabaseStatus {
  state: DatabaseConnectionState;
  message: string;
}

interface BackendTipPosition {
  positionNumber: number;
  finalDistance: string | null;
  retryCount: number | null;
  result: "OK" | "NG" | "SKIPPED";
}

export interface BackendMeasurement {
  id: number;
  productCode: string;
  clubNumber: string;
  measuredAt: string;
  weight: string;
  tipPositions: BackendTipPosition[];
  tipResult: "OK" | "NG" | "INCOMPLETE";
  buttDistance: string;
  buttResult: "OK" | "NG" | "INCOMPLETE";
  overallResult: "OK" | "NG" | "INCOMPLETE";
}

export interface MeasurementListRequest {
  page?: number;
  pageSize?: number;
  search?: string;
  productCode?: string;
  clubNumber?: string;
  measuredFrom?: string;
  measuredTo?: string;
  tipResult?: "OK" | "NG";
  buttResult?: "OK" | "NG";
  result?: "OK" | "NG";
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
  daily: Array<{ date: string; total: number; ok: number; ng: number }>;
  stationResults: Array<{ station: string; ok: number; ng: number }>;
  overallDistribution: Array<{ result: "OK" | "NG" | "INCOMPLETE"; count: number }>;
}

interface CreateMeasurementPayload {
  productCode: string;
  clubNumber: string;
  measuredAt: string;
  weight: string;
  tipPositions: Array<{
    positionNumber: number;
    finalDistance: string | null;
    retryCount: number | null;
    result: "OK" | "NG" | "SKIPPED";
  }>;
  buttDistance: string;
  buttResult: "OK" | "NG" | "INCOMPLETE";
}

function toBackendTip(tip: TipPositionMeasurement): CreateMeasurementPayload["tipPositions"][number] {
  return {
    positionNumber: tip.positionNumber,
    finalDistance: tip.finalDistance === null ? null : String(tip.finalDistance),
    retryCount: tip.retryCount,
    result: tip.result === "INCOMPLETE" ? "SKIPPED" : tip.result,
  };
}

function toCreatePayload(input: MeasurementInput): CreateMeasurementPayload {
  return {
    productCode: input.productCode,
    clubNumber: input.clubNumber,
    measuredAt: input.measurementTime,
    weight: String(input.weight),
    tipPositions: input.tips.map(toBackendTip),
    buttDistance: String(input.butt.finalDistance),
    buttResult: input.butt.result ?? "INCOMPLETE",
  };
}

export function checkDatabaseConnection(): Promise<string> {
  return invoke<string>("check_database_connection");
}

export function getDatabaseStatus(): Promise<DatabaseStatus> {
  return invoke<DatabaseStatus>("get_database_status");
}

export function retryDatabaseConnection(): Promise<DatabaseStatus> {
  return invoke<DatabaseStatus>("retry_database_connection");
}

export function createMeasurement(input: MeasurementInput): Promise<number> {
  return invoke<number>("create_measurement", { request: toCreatePayload(input) });
}

export function getMeasurements(request: MeasurementListRequest = {}): Promise<MeasurementListResponse> {
  return invoke<MeasurementListResponse>("get_measurements", { request });
}

export function getMeasurementById(id: number): Promise<BackendMeasurement> {
  return invoke<BackendMeasurement>("get_measurement_by_id", { id });
}

export function getMeasurementStatistics(request: MeasurementListRequest = {}): Promise<MeasurementStatistics> {
  return invoke<MeasurementStatistics>("get_measurement_statistics", { request });
}
