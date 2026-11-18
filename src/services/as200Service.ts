
import { invoke } from "@tauri-apps/api/core";

export interface As200Latest {
  qualityError?: string | null;
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
  readTelemetry?: { date: string; observedSince: string; readErrors: number; disconnects: number } | null;
  controlSnapshot?: { heartbeat: number; total_count: number; data_ready: number; pc_read_count: number; pc_read_done: number } | null;
  connectedAt?: string | null;
  connectionEndedAt?: string | null;
  lastReadAt?: string | null;
  lastReadMs?: number | null;
  lastError?: string | null;
  connectionEvents?: { at: string; kind: "CONNECTED" | "RECONNECTED" | "DISCONNECTED" | "FAILED" | "MANUAL_STOP"; endpoint: string; message: string }[];
  connectionLogError?: string | null;
  blockedInboxCount: number;
  blockedInboxIds: number[];
  retryWaitingCount: number;
  plcHealth: "UNCONFIGURED" | "OBSERVING" | "RUNNING" | "STALE" | "DISCONNECTED" | "INVALID_DATA";
  pcHeartbeatStatus: string;
  ackStatus:
    | "READ_ONLY_PROTOCOL_UNCONFIRMED"
    | "DISABLED"
    | "WAITING"
    | "PENDING"
    | "ACKNOWLEDGED"
    | "BLOCKED_INVALID_RECORD"
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
  latest: As200Latest | null;
  lastSavedId: number | null;
}

export const getAs200WorkerStatus = () =>
  invoke<As200Status>("get_as200_worker_status");

export const reconnectAs200Plc = () => invoke<void>("reconnect_as200_plc");

export const retryAs200Inbox = (inboxId: number) => invoke<void>("retry_as200_inbox", { inboxId });
