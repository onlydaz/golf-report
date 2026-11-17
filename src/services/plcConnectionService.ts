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
export const savePlcPollInterval = (pollIntervalMs: number) =>
  invoke<PlcConnectionSettings>("save_plc_poll_interval", { pollIntervalMs });
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
export interface PlcRegisterWordsReport {
  endpoint: PlcEndpoint;
  plcLocalIp: string | null;
  modbusAddressOffset: number;
  startD: number;
  words: number[];
  readAt: string;
}
export const readPlcRegisterWords = (startD: number, wordCount: number) =>
  invoke<PlcRegisterWordsReport>("read_plc_register_words", { startD, wordCount });
export const connectPlcCollection = () => invoke<void>("connect_plc_collection");
export const stopPlcCollection = () => invoke<void>("stop_plc_collection");

export interface PlcRegisterSelection {
  endpoint: PlcEndpoint;
  kind: "BOOL" | "WORD";
  address: string;
}
export interface PlcRegisterAccess {
  address: string;
  pduAddress: number;
  addressOffset: number;
  deviceModel: string;
  programId: string;
  readFunction: number;
  writeFunction: number;
  allowedValues: number[];
  writeAllowed: boolean;
  writeBlockReason: string | null;
}
export interface PlcRegisterRead { access: PlcRegisterAccess; value: number }
export interface PlcRegisterWriteReport {
  access: PlcRegisterAccess;
  before: number;
  requested: number;
  after: number | null;
  writeStatus: "ECHO_CONFIRMED" | "REFUSED" | "UNCONFIRMED";
  readbackMatches: boolean | null;
  message: string;
}
export const getPlcRegisterUiEnabled = () => invoke<boolean>("get_plc_register_ui_enabled");
async function requirePlcRegisterUiEnabled() {
  if (!await getPlcRegisterUiEnabled()) {
    throw "Công cụ đọc/ghi thanh ghi kỹ thuật đang tắt.";
  }
}
export const getPlcRegisterAccess = async (selection: PlcRegisterSelection) => {
  await requirePlcRegisterUiEnabled();
  return invoke<PlcRegisterAccess>("get_plc_register_access", { selection });
};
export const readPlcRegister = async (selection: PlcRegisterSelection) => {
  await requirePlcRegisterUiEnabled();
  return invoke<PlcRegisterRead>("read_plc_register", { selection });
};
export const writePlcRegister = async (request: {
  selection: PlcRegisterSelection;
  value: number;
  expectedBefore: number;
  programId: string;
  mappingConfirmed: boolean;
  deviceConfirmed: boolean;
  machineSafeConfirmed: boolean;
  pduAddress: number;
  addressOffset: number;
}) => {
  await requirePlcRegisterUiEnabled();
  return invoke<PlcRegisterWriteReport>("write_plc_register", { request });
};
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
