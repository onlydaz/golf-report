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
export const testPlcConnection = (value: PlcEndpoint) => invoke<void>("test_plc_connection", { value });
export const stopPlcCollection = () => invoke<void>("stop_plc_collection");
export const connectPlcCollection = (value: PlcEndpoint, expectedRevision: string, resetConfirmed = false) =>
  invoke<void>("connect_plc_collection", { value, expectedRevision, ...(resetConfirmed ? { resetConfirmed: true } : {}) });
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
  if (error === "RESET_CONFIRMATION_REQUIRED") return "PLC đã khởi động lại. Vui lòng kiểm tra kết quả đo trước khi tiếp tục.";
  if (/SourceKey|Session|Journal|Boot ID|pause=|quarantine=/i.test(error)) {
    return "Lịch sử nguồn đo cần được kỹ thuật đối chiếu trước khi kết nối lại.";
  }
  if (/ACK|ReadCount|PC_ReadDone|Trạng thái xác nhận|receipt|Inbox|dữ liệu chưa xử lý/i.test(error)) {
    return "Còn dữ liệu hoặc xác nhận đọc cần đối chiếu. Vui lòng liên hệ kỹ thuật.";
  }
  if (/bản ghi đối chiếu|phiên cũ|Bộ đếm|Counter|baseline/i.test(error)) {
    return "Chưa đủ bằng chứng để tiếp tục phiên máy đo cũ. Vui lòng liên hệ kỹ thuật.";
  }
  if (/snapshot|Không thể xác minh dữ liệu máy đo|Dữ liệu máy đo có mâu thuẫn/i.test(error)) {
    return "Chưa thể xác minh dữ liệu hiện tại của máy đo. Vui lòng kiểm tra kết nối và thử lại.";
  }
  if (error === "Không thể lưu thay đổi. Vui lòng thử lại hoặc liên hệ kỹ thuật." || /Configuration changed during save/i.test(error)) {
    return "Không thể lưu cấu hình kết nối. Vui lòng thử lại hoặc liên hệ kỹ thuật.";
  }
  return /^(Địa chỉ|Cổng kết nối|Unit ID|Cấu hình|Chưa|Không thể|Thông tin|Đang|Máy đo|Lịch sử máy đo|Thiếu cấu hình|Thiếu lịch sử|Nguồn đo|Dữ liệu máy đo)/.test(error) ? error : fallback;
}
