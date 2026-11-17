import { useOutletContext } from "react-router-dom";
import type { As200Status } from "../services/as200Service";
import type { PlcConnectionSettings } from "../services/plcConnectionService";

export interface PlcMonitorContext {
  status: As200Status | undefined;
  settings: PlcConnectionSettings | undefined;
  error: string;
  databaseConnected: boolean;
  refresh: () => Promise<As200Status>;
}

// MainLayout owns the existing status polling loop. Pages and headings share it.
export const usePlcMonitor = () => useOutletContext<PlcMonitorContext>();

// Shared labels use backend evidence only; TCP and heartbeat never prove ACK.
export function getPlcDisplayStatus(status: As200Status | undefined, error: string) {
  const invalid = status?.plcHealth === "INVALID_DATA" || status?.state === "ERROR" || !!status?.pauseReason;
  const disconnected = status?.connection === "DISCONNECTED" || status?.plcHealth === "DISCONNECTED";
  const verified = !error && !invalid && status?.connection === "CONNECTED" && status.plcHealth === "RUNNING" && !!status.sessionId && !status.pauseReason;
  const plcLabel = error ? "PLC chưa xác minh" : disconnected ? "Mất kết nối PLC"
    : invalid ? "PLC cần kiểm tra" : verified ? "PLC đã kết nối"
    : status?.plcHealth === "STALE" ? "Tín hiệu PLC đứng yên"
    : status?.enabled ? "Đang xác minh PLC" : "Chưa kết nối PLC";
  const failed = !!error || !!status?.pauseReason || !!status && (
    ["ERROR", "FAILED", "PAUSED", "SESSION_UNCONFIRMED"].includes(status.state)
    || ["FAILED", "RECONCILIATION_REQUIRED"].includes(status.ackStatus)
    || status.pcHeartbeatStatus === "FAILED" || status.latest?.saveState === "FAILED" || status.blockedInboxCount > 0
    || status.enabled && (disconnected || status.plcHealth === "STALE" || status.plcHealth === "INVALID_DATA")
  );
  const collectionLabel = failed ? "Có lỗi" : status?.state === "RAW_UNDECODED" ? "Đã nhận raw, chờ giải mã" : !status ? "Chưa xác định" : status.state === "STOPPING" ? "Đang dừng"
    : !status?.enabled ? "Đã dừng" : verified ? "Đang thu dữ liệu" : "Chờ xác minh / kết nối";
  return { verified, failed, plcLabel, collectionLabel,
    plcColor: verified ? "green" : error || disconnected || invalid ? "red" : "default",
    collectionColor: failed ? "red" : status?.enabled && verified ? "green" : "default",
  };
}
