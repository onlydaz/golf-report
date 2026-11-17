import { invoke } from "@tauri-apps/api/core";

export interface PlcEndpoint { ip: string; port: number; unitId: number }
export interface PlcRuntimeParameters {
  plcType: string;
  protocol: string;
  layoutProfile: string;
  controlStart: number;
  bufferCount: number;
  recordSize: number;
  wordOrder: "LOW" | "HIGH";
  modbusAddressOffset: number;
  pollIntervalMs: number;
  timeoutMs: number;
  heartbeatTimeoutMs: number | null;
  retryIntervalMs: number;
  readOnly: boolean;
  ackEnabled: boolean;
}
export interface PlcConnectionSettings {
  operatorSku?: string | null;
  plcLocalIp?: string | null;
  parameters: PlcRuntimeParameters;
  endpoint: PlcEndpoint;
  configured: boolean;
}
export const getPlcConnectionSettings = () => invoke<PlcConnectionSettings>("get_plc_connection_settings");
export const saveOperatorSku = (sku: string) => invoke<PlcConnectionSettings>("save_operator_sku", { sku });
export const clearPlcConnectionLog = () => invoke<void>("clear_plc_connection_log");
export const savePlcConnectionOptions = (pollIntervalMs: number, plcLocalIp: string) =>
  invoke<PlcConnectionSettings>("save_plc_connection_options", { pollIntervalMs, plcLocalIp });
export interface PlcInspectionReport {
  tcpConnected: boolean;
  modbusReadSucceeded: boolean;
  as200Valid: boolean;
  mappingWarning: string | null;
  message: string;
}
export const testPlcConnection = () => invoke<PlcInspectionReport>("test_plc_connection");
export const connectPlcCollection = () => invoke<void>("connect_plc_collection");
export const stopPlcCollection = () => invoke<void>("stop_plc_collection");

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
  // Backend diagnostics remain internal; operator messages retain the required action.
  if (/\braw\b|RAW_|240\s*WORD|D25020|D25259|\binbox\b|RecordSeq|TotalCount/i.test(error)) {
    if (/ACK|RESET|CONFLICT|RECONCILIATION|đối chiếu|bộ đếm/i.test(error)) {
      return "Dữ liệu hoặc xác nhận từ PLC cần đối chiếu. Không tự gửi lại ACK; vui lòng liên hệ kỹ thuật.";
    }
    return "Chưa có kết quả đo hợp lệ để hiển thị. Vui lòng liên hệ kỹ thuật nếu tình trạng kéo dài.";
  }
  // Preserve protective warnings before replacing transport/internal errors.
  const protectiveWarnings: [RegExp, string][] = [
    [/reset|bộ đếm.*giảm|counter.*decreas/i, "Bộ đếm máy đo cần đối chiếu sau reset. Chưa thể tiếp tục thu; vui lòng liên hệ kỹ thuật."],
    [/(?:^|[^a-z])ACK(?:[^a-z]|$)/i, "Xác nhận đã đọc từ PLC chưa được đối chiếu. Chưa thể tiếp tục; không tự gửi lại ACK. Vui lòng liên hệ kỹ thuật."],
    [/journal/i, "Không thể xác minh hoặc lưu dữ liệu chờ trong Journal. Dữ liệu có nguy cơ mất; vui lòng giữ ứng dụng mở và liên hệ kỹ thuật."],
    [/session|phiên/i, "Phiên máy đo chưa được đối chiếu. Chưa thể tiếp tục; không tự đổi phiên. Vui lòng liên hệ kỹ thuật."],
  ];
  const warning = protectiveWarnings.find(([pattern]) => pattern.test(error));
  if (warning) return /[À-ỹ]/u.test(error) ? error : warning[1];
  return /[À-ỹ]/u.test(error) ? error : fallback;
}
