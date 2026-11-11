
import { invoke } from "@tauri-apps/api/core";

export interface As200Latest {
  sourceKey: string;
  sessionId: string | null;
  deliveryId: string;
  inboxId: number | null;
  measurementId: number | null;
  recordSeq: number;
  totalCount: number | null;
  productCode: string;
  clubNumber: number;
  receivedAt: string;
  overallResult: string;
  tipResult: string;
  buttResult: string;
  saveState: string;
}

export interface As200Status {
  manualRetryResults: { inboxId: number; state: "PENDING" | "WAITING" | "APPLIED" | "REJECTED"; error: string | null }[];
  blockedInboxCount: number;
  blockedInboxIds: number[];
  retryWaitingCount: number;
  retryDiagnostics: string[];
  plcHealth: "UNCONFIGURED" | "OBSERVING" | "RUNNING" | "STALE" | "DISCONNECTED";
  pcHeartbeatStatus: string;
  ackStatus:
    | "READ_ONLY_PROTOCOL_UNCONFIRMED"
    | "DISABLED"
    | "WAITING"
    | "PENDING"
    | "ACKNOWLEDGED"
    | "RECONCILIATION_REQUIRED"
    | "FAILED";

  enabled: boolean;
  address: string;
  unitId: number;
  sourceKey: string;
  sessionId: string | null;
  connection: string;
  state: string;
  message: string;
  pauseReason: string | null;
  warnings: string[];
  volatileCount: number;
  latest: As200Latest | null;
  lastSavedId: number | null;
}

export const getAs200WorkerStatus = () =>
  invoke<As200Status>("get_as200_worker_status");

export const reconnectAs200Plc = () => invoke<void>("reconnect_as200_plc");

export const acknowledgeAs200BufferGap = () =>
  invoke<void>("acknowledge_as200_buffer_gap");

export const retryAs200Inbox = (inboxId: number) => invoke<void>("retry_as200_inbox", { inboxId });
