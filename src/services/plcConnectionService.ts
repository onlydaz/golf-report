import type { As200Status } from "./as200Service";
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
  parameters: PlcRuntimeParameters;
  endpoint: PlcEndpoint;
  configured: boolean;
}
export const getPlcConnectionSettings = () => invoke<PlcConnectionSettings>("get_plc_connection_settings");
export const saveOperatorSku = (sku: string) => invoke<PlcConnectionSettings>("save_operator_sku", { sku });
export const clearPlcConnectionLog = () => invoke<void>("clear_plc_connection_log");
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

export interface PlcConnectionAttempt {
  startedAt: number;
  error?: string;
  report?: PlcInspectionReport;
}

export function isPlcNetworkError(message: string): boolean {
  return /Lỗi Modbus TCP AS200|kết nối TCP PLC|Hết thời gian kết nối\/đọc AS200/i.test(message);
}

// Connection evidence is separate from ACK/storage health. A successful probe
// never claims that collection is running or that a pending ACK is resolved.
export function getPlcConnectionView(status: As200Status | undefined, action: string | null, attempt?: PlcConnectionAttempt) {
  const pending = !!action;
  const connected = !!status?.enabled && status.connection === "CONNECTED" && !!status.lastReadAt && status.plcHealth !== "INVALID_DATA";
  const recovered = connected && !!attempt && Date.parse(status!.lastReadAt!) > attempt.startedAt;
  const feedback = pending ? undefined : !recovered ? attempt?.error || attempt?.report?.message : undefined;
  const failed = !recovered && (!!attempt?.error || !!attempt?.report && !attempt.report.as200Valid);
  const label = action === "test" ? "Đang kiểm tra PLC" : action === "connect" ? "Đang kết nối PLC"
    : action === "stop" || status?.state === "STOPPING" ? "Đang dừng"
    : status?.enabled && status.connection === "CONNECTING" ? "Đang kết nối PLC"
    : connected && (!failed || recovered) ? "Đã kết nối PLC"
    : status?.enabled && status.connection === "DISCONNECTED" ? "Mất kết nối PLC"
    : failed ? "Kết nối thất bại"
    : attempt?.report?.as200Valid ? "Kết nối PLC hợp lệ" : "Chưa kết nối PLC";
  return { label, connected,
    color: pending ? "default" : connected && (!failed || recovered) || attempt?.report?.as200Valid && !status?.enabled ? "green"
      : failed && !recovered || status?.enabled && status.connection === "DISCONNECTED" ? "red" : "default",
    feedback: feedback ? plcConnectionError(feedback, "Chưa thể xác minh kết nối PLC.") : undefined,
    detail: feedback && attempt?.report?.mappingWarning ? plcConnectionError(attempt.report.mappingWarning, "Cần kỹ thuật kiểm tra cấu hình PLC.") : undefined,
    failed,
  };
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
