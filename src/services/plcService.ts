import { invoke } from "@tauri-apps/api/core";
import type { BackendMeasurement } from "./measurementService";

export interface PlcStatus {
  connection: "CONNECTING" | "CONNECTED" | "DISCONNECTED";
  address: string;
  unit: number;
  phase: "WAITING" | "RECEIVING" | "SAVING" | "SAVED" | "ERROR";
  message: string;
  productCode: string | null;
  sequenceId: number | null;
  data: Omit<BackendMeasurement, "id" | "tipResult" | "overallResult"> | null;
  measurementId: number | null;
  busy: boolean;
}
export const getPlcStatus = () => invoke<PlcStatus>("get_plc_status");
export const startPlcCapture = (productCode: string) => invoke<void>("start_plc_capture", { productCode });
