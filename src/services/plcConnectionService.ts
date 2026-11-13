import { invoke } from "@tauri-apps/api/core";

export interface PlcEndpoint { ip: string; port: number; unitId: number }
export interface PlcConnectionSettings {
  endpoint: PlcEndpoint | null;
  configured: boolean;
  technicalReady: boolean;
  activeEndpoint: PlcEndpoint | null;
  revision: string;
  workerRunning: boolean;
  restartRequired: boolean;
}
export const getPlcConnectionSettings = () => invoke<PlcConnectionSettings>("get_plc_connection_settings");
/** Kết nối + đọc FC03 thành công là "đã kết nối"; `mappingWarning` báo dữ liệu không khớp register map. */
export interface PlcProbeReport { mappingWarning: string | null }
export const testPlcConnection = (value: PlcEndpoint) => invoke<PlcProbeReport>("test_plc_connection", { value });
export const stopPlcCollection = () => invoke<void>("stop_plc_collection");
export const connectPlcCollection = (value: PlcEndpoint, expectedRevision: string, resetConfirmed = false) =>
  invoke<void>("connect_plc_collection", { value, expectedRevision, resetConfirmed });
export const updatePlcConnectionSettings = (value: PlcEndpoint, expectedRevision: string) =>
  invoke<PlcConnectionSettings>("update_plc_connection_settings", { value, expectedRevision });

export function isPlcIp(value: string): boolean {
  if (value.includes(":")) {
    if (!/^[0-9a-fA-F:]+$/.test(value)) return false;
    try { return new URL(`http://[${value}]/`).hostname.length > 0; } catch { return false; }
  }
  const parts = value.split(".");
  return parts.length === 4 && parts.every((part) => /^(0|[1-9]\d{0,2})$/.test(part) && Number(part) <= 255);
}
export function plcConnectionError(error: unknown, fallback: string): string {
  if (typeof error !== "string") return fallback;
  if (error === "Không thể lưu thay đổi. Vui lòng thử lại hoặc liên hệ kỹ thuật." || /Configuration changed during save/i.test(error)) {
    return "Không thể lưu cấu hình kết nối. Vui lòng thử lại hoặc liên hệ kỹ thuật.";
  }
  return error;
}
