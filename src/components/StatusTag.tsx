
import type { MeasurementStatus } from "../types/measurement";

function StatusTag({ status }: { status: MeasurementStatus }) {
  return (
    <span className={`status-pill ${status === "OK" ? "status-ok" : "status-ng"}`}>
      {status}
    </span>
  );
}

export default StatusTag;
