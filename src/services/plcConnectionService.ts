import { invoke } from "@tauri-apps/api/core";

export interface PlcEndpoint { ip: string; port: number; unitId: number }
export interface PlcRuntimeParameters {
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
  autoReconnect: boolean;
  readOnly: boolean;
  startWithWindows: boolean;
  skuTableStart: number | null;
}
export interface PlcRuntimeSettings { endpoint: PlcEndpoint; parameters: PlcRuntimeParameters }
export const savePlcRuntimeSettings = (value: PlcRuntimeSettings, expectedRevision: string) =>
  invoke<PlcConnectionSettings>("save_plc_runtime_settings", { value, expectedRevision });
export const applyPlcRuntimeSettings = (value: PlcRuntimeSettings, expectedRevision: string, resetConfirmed = false) =>
  invoke<void>("apply_plc_runtime_settings", { value, expectedRevision, resetConfirmed });
export const testPlcRuntimeSettings = (value: PlcRuntimeSettings) =>
  invoke<PlcInspectionReport>("test_plc_runtime_settings", { value });
export interface PlcConnectionSettings {
  parameters: PlcRuntimeParameters;
  activeParameters: PlcRuntimeParameters | null;
  verificationRequired: boolean;
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
export interface PlcInspectionReport extends PlcProbeReport {
  tcpConnected: boolean;
  modbusReadSucceeded: boolean;
  as200Valid: boolean;
  message: string;
}
export interface PlcConnectionOutcome {
  inspection: PlcInspectionReport | null;
  collectionStarted: boolean;
  blockReason: string | null;
}
export const testPlcConnection = (value: PlcEndpoint) => invoke<PlcProbeReport>("test_plc_connection", { value });
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
export interface PlcSourceVerification {
  physicalIdentity: string;
  newSourceConfirmed: boolean;
  registerMapConfirmed: boolean;
  ackWriteApproved: boolean;
}
/** QC never sends physical-source approval or reset/session confirmation. */
export const connectVerifiedPlc = (value: PlcEndpoint, expectedRevision: string) =>
  invoke<PlcConnectionOutcome>("connect_verified_plc", { value, expectedRevision });
export const connectPlcCollection = (value: PlcEndpoint, expectedRevision: string, resetConfirmed = false, sourceVerification?: PlcSourceVerification) =>
  invoke<void>("connect_plc_collection", { value, expectedRevision, resetConfirmed, sourceVerification });
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
