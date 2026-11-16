import { formatReportTime } from "../../utils/reportTime";
import { formatDecimal } from "../../utils/as200Report";
import {
  Alert,
  Button,
  Card,
  Collapse,
  Popconfirm,
  Space,
  Tag,
  message,
  InputNumber,
} from "antd";

import { useEffect, useState } from "react";
import { useRef } from "react";
import { PauseCircleOutlined, PlayCircleOutlined, WifiOutlined } from "@ant-design/icons";
import { getPlcDisplayStatus, usePlcMonitor } from "../../hooks/usePlcMonitor";
import PageHeader from "../../components/PageHeader";
import { BendComparison, CurrentPointTable, ProductionSummary, RecentMeasurements } from "./MonitorPanels";

import {
  acknowledgeAs200BufferGap,
  retryAs200Inbox,
} from "../../services/as200Service";

import MeasurementDetails from "../../components/measurement/MeasurementDetails";
import "./AddMeasurement.css";
import { connectPlcCollection, getPlcConnectionSettings, plcConnectionError, stopPlcCollection } from "../../services/plcConnectionService";
import { reconnectAs200Plc } from "../../services/as200Service";

import {
  getMeasurementById,
  getMeasurements,
  getMeasurementStatistics,
  type BackendMeasurement,
  type MeasurementDetail,
  type MeasurementStatistics,
} from "../../services/measurementService";

function displayNotice(notice: string, fallback: string): string {
  if (notice.startsWith("Chưa thiết lập kết nối")) return "Chưa thiết lập kết nối. Vào Kết nối PLC để nhập thông tin thiết bị đo.";
  if (/Cần kỹ thuật xác nhận thông số|Chưa xác nhận thiết lập kỹ thuật/.test(notice)) return "Chưa bật ghi nhận kết quả đo.";
  if (/SESSION_REQUEST_CANCELLED/.test(notice)) { return "Đã hủy yêu cầu đổi phiên và giữ nguyên dữ liệu, trạng thái xác nhận của phiên cũ."; }
  if (/Đã đối chiếu phiên mới/.test(notice)) { return "Đã đối chiếu và chấp nhận phiên kết nối mới. Dữ liệu phiên cũ vẫn được giữ nguyên."; }
  if (/INBOX_RETRY_BLOCKED/.test(notice)) {
    const id = /inbox=(\d+)/.exec(notice)?.[1];
    return `Kết quả${id ? ` tiếp nhận ${id}` : ""} đang chờ kỹ thuật kiểm tra. Dữ liệu được giữ nguyên; chỉ thử lại sau khi đã xử lý nguyên nhân.`;
  }
  if (/MANUAL_RETRY/.test(notice)) { return "Đã ghi nhận yêu cầu thử lại sau khi kiểm tra dữ liệu; chưa xác nhận đã xử lý thành công."; }
  if (/SESSION_RECONCILIATION/.test(notice)) {
    return "Phiên kết nối mới chưa được chấp nhận. Kỹ thuật cần đối chiếu bộ đếm, dữ liệu chờ và xác nhận cũ; hệ thống giữ nguyên phiên và dữ liệu đang có.";
  }
  if (/DIAGNOSTIC_FULL/.test(notice)) {
    return "Vùng lưu chẩn đoán đã đầy. Hệ thống tạm dừng tiếp nhận để bảo toàn dữ liệu; vui lòng liên hệ kỹ thuật và giữ nguyên dữ liệu chờ.";
  }
  if (/QUARANTINE/.test(notice)) {
    return "Có dữ liệu PLC chưa hợp lệ hoặc chưa hoàn tất. Hệ thống đã giữ dữ liệu để chẩn đoán và chưa xác nhận đã đọc; vui lòng liên hệ kỹ thuật.";
  }
  if (/LOCAL_HISTORY_MISSING/.test(notice)) {
    return "Lịch sử tiếp nhận trên máy không khớp với cơ sở dữ liệu. Vui lòng liên hệ kỹ thuật để đối chiếu. Hệ thống chưa gửi xác nhận đã đọc.";
  }

  if (/ACK chưa được phép/.test(notice)) {
    return "Chưa thể bật xác nhận đã đọc vì quy trình PLC chưa được xác nhận. Vui lòng liên hệ kỹ thuật để kiểm tra cấu hình.";
  }

  if (/Chưa cho phép kết nối PLC thật|Chế độ PLC yêu cầu cấu hình/.test(notice)) {
    return "Chưa cho phép kết nối PLC thật. Vui lòng liên hệ kỹ thuật để xác nhận thông số và cấu hình trước khi kết nối.";
  }

  const saved = /^Đã commit PostgreSQL\. Mã bản ghi: (\d+)$/.exec(notice);

  if (saved) {
    return `Đã lưu kết quả đo thành công. Mã phép đo: ${saved[1]}`;
  }

  if (/RESET\/CONFLICT/.test(notice)) {
    return "Bộ đếm PLC không khớp dữ liệu đã lưu. Hệ thống chưa xác nhận đã đọc; vui lòng liên hệ kỹ thuật để đối chiếu.";
  }

  if (/RESET|counter giảm|TotalCount thấp hơn/.test(notice)) {
    return "Bộ đếm thiết bị đã giảm so với lần tiếp nhận trước. Vui lòng liên hệ kỹ thuật để đối chiếu phiên. Không đổi phiên để bỏ qua cảnh báo.";
  }

  if (/CONFLICT|xung đột|không khớp/.test(notice)) {
    return "Dữ liệu tiếp nhận không khớp với kết quả đã ghi nhận trong cùng phiên. Vui lòng liên hệ kỹ thuật để kiểm tra.";
  }

  if (/BUFFER GAP/.test(notice)) {
    return "Có nguy cơ thiếu kết quả đo do bộ nhớ PLC đã ghi đè. Hệ thống chỉ giữ được các kết quả còn đọc được; vui lòng kiểm tra trước khi tiếp tục.";
  }

  if (/BASELINE/.test(notice)) {
    return "Chỉ tiếp nhận được các kết quả còn trong bộ nhớ PLC. Chưa thể xác nhận đầy đủ kết quả trước khi hệ thống bắt đầu theo dõi.";
  }

  if (/Hàng chờ đầy/.test(notice)) {
    return "Hàng chờ dữ liệu đã đầy, hệ thống tạm dừng tiếp nhận. Vui lòng kiểm tra việc lưu dữ liệu và bộ nhớ PLC trước khi tiếp tục.";
  }

  if (/Người dùng đã đổi session/.test(notice)) {
    return "Phiên kết nối đã thay đổi. Phiên mới chỉ áp dụng cho các kết quả tiếp nhận tiếp theo; dữ liệu cũ vẫn cần được xác nhận riêng.";
  }

  if (/Người dùng xác nhận đã kiểm tra buffer gap/.test(notice)) {
    return "Đã xác nhận kiểm tra nguy cơ thiếu kết quả đo. Các kết quả đã mất không được khôi phục.";
  }

  if (/Đã xác nhận buffer gap/.test(notice)) {
    return "Đã xác nhận kiểm tra dữ liệu, tiếp tục theo dõi kết quả đo. Cảnh báo trước đó vẫn được lưu.";
  }

  if (/không khóa được|giữ khóa/.test(notice)) {
    return "Không thể mở dữ liệu chờ lưu. Vui lòng kiểm tra ứng dụng có đang chạy ở cửa sổ khác hoặc liên hệ kỹ thuật.";
  }

  if (/Journal|journal/.test(notice)) {
    return "Không thể đọc hoặc lưu dữ liệu chờ trên máy tính. Kết quả chưa lưu có nguy cơ bị mất khi đóng ứng dụng; vui lòng liên hệ kỹ thuật.";
  }

  if (/chưa được bật/.test(notice)) {
    return "Hệ thống đo tự động chưa được bật. Vui lòng liên hệ kỹ thuật để kiểm tra cấu hình.";
  }

  if (/Đang khởi động worker/.test(notice)) {
    return "Đang khởi động hệ thống tiếp nhận kết quả đo...";
  }

  if (/Worker đã tạm dừng/.test(notice)) {
    return "Hệ thống đã tạm dừng tiếp nhận. Vui lòng kiểm tra cảnh báo; các kết quả đã nhận vẫn được thử lưu lại.";
  }

  if (/Đã nhận bản ghi hoàn thành/.test(notice)) {
    return "Đã nhận kết quả đo, đang chờ lưu dữ liệu.";
  }

  if (/đang chờ bản ghi hoàn thành/.test(notice)) {
    return "Đã kết nối PLC, đang chờ kết quả đo hoàn tất.";
  }

  if (/AS200 đã kết nối lại/.test(notice)) {
    return "Đã kết nối lại PLC, tiếp tục theo dõi kết quả đo.";
  }

  if (/chưa xác nhận phiên/.test(notice)) {
    return "Đã tiếp nhận dữ liệu nhưng chưa xác nhận phiên kết nối. Kết quả đo chưa được xử lý.";
  }

  if (/chưa phải bản ghi hoàn thành/.test(notice)) {
    return "Kết quả đo chưa hoàn tất, chưa thể xử lý và lưu phép đo.";
  }

  if (/Hết thời gian PostgreSQL|pool|database|PostgreSQL|SQL|inbox thất bại/i.test(notice)) {
    if (notice === "Đã lưu inbox, đang chờ xử lý PostgreSQL.") {
      return "Đã tiếp nhận dữ liệu, đang chờ xử lý kết quả đo.";
    }

    if (notice.startsWith("Đang xử lý inbox")) {
      return "Đang xử lý và lưu kết quả đo...";
    }

    return "Chưa lưu được kết quả đo. Hệ thống sẽ thử lại; vui lòng kiểm tra kết nối lưu dữ liệu nếu lỗi kéo dài.";
  }

  if (/Đã xử lý các inbox đủ điều kiện/.test(notice)) {
    return "Đã xử lý các dữ liệu đủ điều kiện. Vui lòng xem trạng thái lưu của kết quả đo gần nhất.";
  }

  if (/timeout|transport|connection|kết nối|Modbus|TCP/i.test(notice)) {
    return "Không thể đọc dữ liệu PLC. Vui lòng kiểm tra kết nối; hệ thống sẽ thử kết nối lại.";
  }

  if (/snapshot|record|decode|giải mã|WORD|WordOrder|Status|format/i.test(notice)) {
    return "Dữ liệu nhận từ PLC chưa hợp lệ hoặc chưa nhất quán. Vui lòng liên hệ kỹ thuật để kiểm tra.";
  }

  return fallback;
}

const resultLabel = (value?: string | null) => value === "SKIPPED" ? "BỎ QUA"
  : value === "UNMEASURED" ? "CHƯA ĐO" : value || "—";
const resultTone = (value?: string | null) => value === "OK" ? "ok"
  : value === "NG" ? "ng" : "empty";

const states: Record<string, string> = {
  STOPPING: "Đang dừng thu dữ liệu",
  STOPPED: "Đã dừng thu dữ liệu",
  SETUP_REQUIRED: "Chờ xác nhận thiết lập",
  DISABLED: "Chưa bật",
  CONNECTING: "Đang kết nối",
  WAITING: "Đang chờ kết quả",
  QUEUED: "Đang chờ lưu dữ liệu",
  PENDING: "Đã tiếp nhận, chờ xử lý",
  SAVING: "Đang lưu dữ liệu",
  PROCESSED: "Đã lưu thành công",
  SESSION_UNCONFIRMED: "Chưa xác nhận phiên",
  FAILED: "Lỗi lưu dữ liệu, sẽ thử lại",
  ERROR: "Có lỗi",
  PAUSED: "Đã tạm dừng",
};

function AddMeasurement() {
  const { status, settings, error, databaseConnected, refresh: refreshWorker } = usePlcMonitor();
  const [collectionError, setCollectionError] = useState("");
  const [collectionAction, setCollectionAction] = useState<"stop" | "start" | "reconnect" | null>(null);
  const collectionInFlight = useRef(false);
  const [retryId, setRetryId] = useState<number | null>(null);
  const [messageApi, holder] = message.useMessage();
  const [recent, setRecent] = useState<BackendMeasurement[]>([]);
  const [statistics, setStatistics] = useState<{ productCode: string; value: MeasurementStatistics }>();
  const [monitorLoading, setMonitorLoading] = useState(false);
  const [monitorError, setMonitorError] = useState("");
  const [refreshIndex, setRefreshIndex] = useState(0);
  const currentSku = status?.latest
    && status.latest.sourceKey === status.sourceKey
    && status.latest.sessionId === status.sessionId
    && status.latest.productCode.trim()
    ? status.latest.productCode : undefined;
  const skuStatistics = databaseConnected && currentSku && statistics?.productCode === currentSku
    ? statistics.value : undefined;

  // Refresh stored data on the existing Worker's commit signal, not a second
  // polling timer. Read-only API calls do not receive or process PLC deliveries.
  useEffect(() => {
    let active = true;
    setStatistics(undefined);
    if (!databaseConnected) {
      setMonitorLoading(false);
      setMonitorError("Chưa thể cập nhật dữ liệu đã lưu. Vui lòng kiểm tra kết nối cơ sở dữ liệu.");
      return;
    }
    setMonitorLoading(true); setMonitorError("");
    void (async () => {
      try {
        const response = await getMeasurements({ page: 1, pageSize: 12 });
        if (!active) return;
        setRecent(response.items);
        if (currentSku) {
          const stats = await getMeasurementStatistics({ productCode: currentSku });
          if (active) setStatistics({ productCode: currentSku, value: stats });
        }
      } catch {
        if (active) setMonitorError("Không tải được dữ liệu giám sát đã lưu. Vui lòng thử làm mới.");
      } finally { if (active) setMonitorLoading(false); }
    })();
    return () => { active = false; };
  }, [databaseConnected, status?.lastSavedId, currentSku, refreshIndex]);

  useEffect(() => {
    if (status?.enabled && status.connection === "CONNECTED" && !status.pauseReason) {
      setCollectionError("");
    }
  }, [status]);

  const latest = status?.latest;
  const displayed = latest ?? recent[0];
  const stoppedWithPendingData = status?.state === "STOPPED" && status.message.includes("dữ liệu chưa lưu");
  const safelyStopped = status?.state === "STOPPED" && !status.enabled && !stoppedWithPendingData;
  const collectionLabel = collectionAction ? collectionAction === "stop" ? "Đang dừng" : "Đang kết nối lại"
    : !status ? "Chưa xác định" : status.state === "STOPPING" ? "Đang dừng"
      : stoppedWithPendingData ? "Đã dừng; còn dữ liệu chờ xử lý"
        : safelyStopped ? "Đã dừng" : status.pauseReason || status.state === "ERROR" ? "Cần kiểm tra"
          : status.connection === "DISCONNECTED" ? "Mất kết nối PLC"
            : status.enabled && status.connection === "CONNECTED" ? "Đang thu" : status.enabled ? "Đang kết nối lại" : "Chưa bắt đầu";
  const changeCollection = async (action: "stop" | "start" | "reconnect") => {
    if (collectionInFlight.current) return;
    collectionInFlight.current = true;
    setCollectionAction(action);
    setCollectionError("");
    try {
      if (action === "stop") await stopPlcCollection();
      else if (action === "reconnect") await reconnectAs200Plc();
      else {
        const current = await getPlcConnectionSettings();
        if (!current.configured || !current.endpoint || current.restartRequired) {
          throw "Chưa có cấu hình kết nối có thể tiếp tục an toàn.";
        }
        await connectPlcCollection(current.endpoint, current.revision);
      }
      const next = await refreshWorker();
      if (action === "reconnect" && (next.connection !== "CONNECTED" || next.pauseReason)) {
        throw "Chưa thể đọc lại PLC an toàn.";
      }
    } catch (reason) {
      console.error("PLC collection operation failed:", reason);
      await refreshWorker().catch(() => undefined);
      setCollectionError(plcConnectionError(reason, action === "stop"
        ? "Chưa thể dừng thu dữ liệu an toàn. Vui lòng kiểm tra dữ liệu đang chờ."
        : "Chưa thể bắt đầu thu dữ liệu. Vui lòng kiểm tra kết nối và dữ liệu đang chờ."));
    } finally {
      collectionInFlight.current = false;
      setCollectionAction(null);
    }
  };
  const showSystemNotice = !!error || !!status && (
    !status.enabled || ["STOPPING", "ERROR", "FAILED", "PAUSED", "SESSION_UNCONFIRMED"].includes(status.state) || !!status.pauseReason ||
    status.connection === "DISCONNECTED" ||
    status.enabled && status.plcHealth !== "RUNNING" ||
    status.ackStatus === "FAILED" || status.ackStatus === "RECONCILIATION_REQUIRED" ||
    status.pcHeartbeatStatus === "FAILED" || latest?.saveState === "FAILED"
  );
  const resetNeedsReview = !error && status?.pauseReason === "RESET";
  const hasIndependentDataRisk = !!status && (
    status.volatileCount > 0 || status.blockedInboxCount > 0 || latest?.saveState === "FAILED" ||
    status.ackStatus === "FAILED" || status.ackStatus === "RECONCILIATION_REQUIRED" ||
    status.warnings.some((warning) => /BUFFER GAP|OVERFLOW|QUEUE_FULL|CONFLICT|LOCAL_HISTORY_MISSING|DIAGNOSTIC_FULL|INBOX_RETRY_BLOCKED/.test(warning))
  );
  const resetConfirmationReady = resetNeedsReview && collectionError === "RESET_CONFIRMATION_REQUIRED" && !hasIndependentDataRisk;
  let systemNotice = error;
  if (!systemNotice && status) {
    if (resetNeedsReview) {
      systemNotice = status.ackStatus === "RECONCILIATION_REQUIRED"
        ? "ACK chưa hoàn tất; cần kỹ thuật đối chiếu xác nhận từ PLC."
        : status.ackStatus === "FAILED"
          ? "Không xác nhận được ACK với PLC. Vui lòng liên hệ kỹ thuật."
          : latest?.saveState === "FAILED"
            ? "Chưa lưu được kết quả đo gần nhất. Vui lòng kiểm tra kết nối dữ liệu."
            : "Cần kiểm tra kết nối máy đo";
    } else if (status.ackStatus === "RECONCILIATION_REQUIRED") {
      systemNotice = "Cần kỹ thuật đối chiếu xác nhận từ PLC. Dữ liệu chờ được giữ nguyên.";
    } else if (status.ackStatus === "FAILED") {
      systemNotice = "Chưa nhận được xác nhận đã đọc từ PLC. Vui lòng liên hệ kỹ thuật nếu cảnh báo kéo dài.";
    } else if (["ERROR", "FAILED", "PAUSED", "SESSION_UNCONFIRMED"].includes(status.state) || status.pauseReason) {
      systemNotice = displayNotice(status.message, states[status.state] ?? "Có lỗi cần kiểm tra.");
    } else if (latest?.saveState === "FAILED") {
      systemNotice = "Chưa lưu được kết quả đo gần nhất. Hệ thống sẽ thử lại; vui lòng kiểm tra kết nối dữ liệu.";
    } else if (status.state === "STOPPING") {
      systemNotice = "Hệ thống đang dừng thu dữ liệu.";
    } else if (!status.enabled) {
      systemNotice = "Hệ thống đã dừng thu dữ liệu. Có thể bắt đầu thu tại đây khi đủ điều kiện.";
    } else if (status.connection === "DISCONNECTED") {
      systemNotice = "Mất kết nối với máy đo. Đang thử kết nối lại...";
    } else if (status.pcHeartbeatStatus === "FAILED") {
      systemNotice = "Chưa gửi được tín hiệu hoạt động từ phần mềm tới PLC. Vui lòng kiểm tra kết nối.";
    } else if (status.plcHealth === "STALE") {
      systemNotice = "Tín hiệu hoạt động PLC đã đứng yên. Hệ thống chưa gửi xác nhận dữ liệu.";
    } else if (status.plcHealth === "UNCONFIGURED") {
      systemNotice = "Chưa cấu hình giám sát hoạt động PLC. Vui lòng liên hệ kỹ thuật.";
    } else if (status.plcHealth === "INVALID_DATA") {
      systemNotice = `Đã kết nối PLC nhưng dữ liệu không khớp register map AS200: ${status.message}`;
    } else if (status.plcHealth === "DISCONNECTED") {
      systemNotice = "Không kiểm tra được tín hiệu hoạt động PLC. Đang thử kết nối lại.";
    } else {
      systemNotice = "Đang theo dõi hoạt động PLC trước khi xác nhận dữ liệu.";
    }
  }
  const visibleWarnings = (status?.warnings ?? []).filter((warning) =>
    !/^(?:RESET_CONTEXT:|RESET:)/.test(warning) && !(resetConfirmationReady && /^QUARANTINE:/.test(warning))
  ).map((warning) => ({
    message: displayNotice(warning, "Hệ thống ghi nhận cảnh báo. Vui lòng liên hệ kỹ thuật để kiểm tra."),
    severity: (/BUFFER GAP|OVERFLOW|QUEUE_FULL|CONFLICT|LOCAL_HISTORY_MISSING|DIAGNOSTIC_FULL|QUARANTINE|INBOX_RETRY_BLOCKED/.test(warning) ? "error" : "warning") as "error" | "warning",
  })).filter((warning, index, all) => warning.message !== systemNotice &&
    all.findIndex((other) => other.message === warning.message && other.severity === warning.severity) === index);
  const hasSpecificPauseError = !!status?.pauseReason && (
    systemNotice === "Cần kiểm tra kết nối máy đo" ||
    systemNotice === "Hệ thống đã tạm dừng tiếp nhận. Vui lòng kiểm tra cảnh báo; các kết quả đã nhận vẫn được thử lưu lại."
  ) && visibleWarnings.some((warning) => warning.severity === "error");
  const resetActionError = resetNeedsReview && collectionError !== "RESET_CONFIRMATION_REQUIRED" &&
    /reset|counter|phiên|ACK|Journal|quarantine/i.test(collectionError);
  const pendingStorageFailure = !!status?.volatileCount && (
    ["ERROR", "FAILED", "PAUSED", "STOPPING", "STOPPED"].includes(status.state) ||
    status.connection === "DISCONNECTED" || !!status.pauseReason
  );
  const journalWriteFailure = !!status?.volatileCount && /Không lưu được journal|Journal lỗi|Không thể đọc hoặc lưu dữ liệu chờ/i.test(status.message);
  const duplicateStorageNotice = !error && !!status?.volatileCount &&
    status.ackStatus !== "FAILED" && status.ackStatus !== "RECONCILIATION_REQUIRED" &&
    /Hết thời gian PostgreSQL|pool|database|inbox thất bại|Không lưu được journal|Journal lỗi/i.test(status.message) &&
    ["ERROR", "FAILED"].includes(status.state);
  const committedId = latest ? latest.measurementId : recent[0]?.id;

  const [detail, setDetail] = useState<MeasurementDetail>();
  const [detailError, setDetailError] = useState("");
  const [detailLoading, setDetailLoading] = useState(false);

  useEffect(() => {
    let active = true;

    setDetail(undefined);
    setDetailError("");
    setDetailLoading(!!committedId && databaseConnected);

    if (committedId && databaseConnected) {
      void getMeasurementById(committedId)
        .then((value) => {
          if (active) {
            setDetail(value);
          }
        })
        .catch(() => {
          if (active) {
            setDetailError(
              "Không tải được chi tiết kết quả đo đã lưu. Vui lòng kiểm tra kết nối dữ liệu."
            );
          }
        })
        .finally(() => {
          if (active) {
            setDetailLoading(false);
          }
        });
    }

    return () => {
      active = false;
    };
  }, [committedId, databaseConnected]);

  const acknowledge = async () => {
    try {
      await acknowledgeAs200BufferGap();

      messageApi.info(
        "Đã gửi xác nhận kiểm tra dữ liệu. Cảnh báo vẫn được lưu trong nhật ký."
      );
    } catch {
      messageApi.error(
        "Không thể tiếp tục tiếp nhận kết quả đo. Vui lòng kiểm tra cảnh báo."
      );
    }
  };

  const displayStatus = getPlcDisplayStatus(status, error);
  const activeDetail = detail && detail.id === committedId ? detail : undefined;
  const receivedTime = latest?.receivedAt ?? recent[0]?.measuredAt;

  return (
    <div className="monitor-page">
      {holder}

      <PageHeader eyebrow="GIÁM SÁT" title="1 · Giám sát" description="" />

      <div className="monitor-top-grid">
      <ProductionSummary productCode={currentSku}
        statistics={skuStatistics} loading={monitorLoading} />
      <Card className="section-card add-measurement-plc-card">
        <div className="add-measurement-plc-main">
          <div className="add-measurement-plc-copy">
            <div className="monitor-kicker">Trạng thái máy</div>
            <strong className={`monitor-machine-state${!collectionAction && !stoppedWithPendingData && displayStatus.collectionLabel === "Đang thu dữ liệu" ? " monitor-machine-state-running" : ""}`}>{collectionAction || stoppedWithPendingData ? collectionLabel : displayStatus.collectionLabel}</strong>
          </div>
          <div className="add-measurement-plc-controls">
            <Tag color={collectionLabel === "Đang thu" ? "green" : collectionLabel === "Cần kiểm tra" || status?.connection === "DISCONNECTED" ? "red" : collectionLabel === "Đã dừng" ? "default" : "orange"}>{collectionLabel}</Tag>
            {status?.enabled && status.state !== "STOPPING" &&
              <Button danger icon={<PauseCircleOutlined />} loading={collectionAction === "stop"} disabled={!!collectionAction} onClick={() => void changeCollection("stop")}>Dừng thu dữ liệu</Button>}
            {status?.enabled && status.connection === "DISCONNECTED" && !status.pauseReason &&
              <Button icon={<WifiOutlined />} loading={collectionAction === "reconnect"} disabled={!!collectionAction} onClick={() => void changeCollection("reconnect")}>Kết nối lại PLC</Button>}
            {safelyStopped && settings?.configured && settings.endpoint && !settings.restartRequired &&
              <Button type="primary" icon={<PlayCircleOutlined />} loading={collectionAction === "start"} disabled={!!collectionAction} onClick={() => void changeCollection("start")}>Bắt đầu thu dữ liệu</Button>}
          </div>
        </div>
        {collectionError && collectionError !== "RESET_CONFIRMATION_REQUIRED" && <Alert className="add-measurement-plc-error" type="error" showIcon message={collectionError} />}
        {resetConfirmationReady && <Alert className="add-measurement-plc-error" type="warning" showIcon message="Cần xác nhận phiên đo" description="Bộ đếm máy đo đã được đặt lại. Vui lòng xác nhận phiên đo mới để tiếp tục." />}
        {!collectionError && status?.enabled && status.connection === "DISCONNECTED" && status.pauseReason !== "RESET" && <Alert className="add-measurement-plc-error" type="error" showIcon message="Mất kết nối với máy đo. Đang thử kết nối lại..." />}
      </Card>
      </div>

      <div className="monitor-result-grid">
      <Card className="section-card add-measurement-results">
        <div className="monitor-current-heading">
          <h2>Gậy vừa kiểm tra · {displayed ? `#${displayed.clubNumber}` : "Chưa có dữ liệu"}</h2>
          <span>{receivedTime ? formatReportTime(receivedTime) : "—"} · Load Butt {formatDecimal(activeDetail?.stations.find(station => station.stationType === "BUTT")?.load)} g / Tip {formatDecimal(activeDetail?.stations.find(station => station.stationType === "TIP")?.load)} g</span>
        </div>
        <div className="add-measurement-result-grid">
          {(["BUTT", "TIP", "OVERALL"] as const).map((station) => {
            const value = station === "TIP" ? displayed?.tipResult
              : station === "BUTT" ? displayed?.buttResult : displayed?.overallResult;
            return <div className={`add-measurement-result add-measurement-result-${resultTone(value)} ${station === "OVERALL" ? "add-measurement-result-overall" : ""}`} key={station}>
              <span>{station}</span>
              <strong>{resultLabel(value)}</strong>
            </div>;
          })}
        </div>
        {latest && (latest.saveState !== "PROCESSED" || latest.measurementId == null || !!latest.inboxId && status?.blockedInboxIds.includes(latest.inboxId)) && <div className="monitor-save-state">Trạng thái lưu dữ liệu <Tag color={latest.saveState === "FAILED" ? "red" : "orange"}>
          {latest.inboxId && status?.blockedInboxIds.includes(latest.inboxId)
            ? "Chờ kỹ thuật kiểm tra" : latest.saveState === "PROCESSED" ? "Đang kiểm tra trạng thái lưu"
              : states[latest.saveState] ?? "Đang cập nhật trạng thái lưu"}
        </Tag></div>}
        {detailError && <Alert type="error" showIcon message={detailError} />}
        <CurrentPointTable detail={activeDetail} loading={detailLoading} />
        <p className="monitor-caption">Điểm NG được đo lại tối đa 4 lần; hiển thị lần đo quyết định.</p>
        {activeDetail && <Collapse className="monitor-full-detail" items={[{ key: "attempts", label: "Xem tất cả lượt đo và thông tin bản ghi", children: <>
          <p className="monitor-caption">SKU {displayed?.productCode} · RecordSeq {displayed?.recordSeq} · TotalCount {displayed?.totalCount ?? "—"} · Phép đo #{committedId}</p>
          <MeasurementDetails data={activeDetail} qcLabels showGeneralInformation={false} />
        </> }]} />}
      </Card>
      <BendComparison />
      </div>

      {showSystemNotice && !resetConfirmationReady && !hasSpecificPauseError && !resetActionError && !duplicateStorageNotice && (status?.connection !== "DISCONNECTED" || !!error || !!status?.pauseReason) && <Alert
        type={error || status?.state === "ERROR" || status?.state === "FAILED" || status?.pauseReason || latest?.saveState === "FAILED" || status?.ackStatus === "FAILED" || status?.ackStatus === "RECONCILIATION_REQUIRED" ? "error" : "warning"}
        showIcon
        className="add-measurement-alert"
        message={systemNotice}
        description={resetNeedsReview && systemNotice === "Cần kiểm tra kết nối máy đo" ? "Dữ liệu từ PLC chưa được xác nhận đầy đủ. Vui lòng liên hệ kỹ thuật trước khi tiếp tục thu." : undefined}
      />}

      {(status?.manualRetryResults?.length ?? 0) > 0 && (
        <Alert type="info" showIcon message="Yêu cầu thử lại kết quả đo"
          description={status?.manualRetryResults.map((r) => (
            <div key={r.inboxId}>Mã tiếp nhận {r.inboxId}: {{ PENDING: "Đang chờ", WAITING: "Chờ kết nối để thử lại", APPLIED: "Đã chuyển lịch xử lý (chưa xác nhận lưu phép đo)", REJECTED: "Yêu cầu bị từ chối" }[r.state]}{r.error ? ` — ${r.error}` : ""}</div>
          ))} />
      )}
      {(status?.blockedInboxCount ?? 0) > 0 && (
        <Alert className="add-measurement-retry-alert" type="warning" showIcon style={{ marginBottom: 16 }}
          message={`${status?.blockedInboxCount} kết quả chờ kỹ thuật kiểm tra trước khi thử lại.`}
          description={<Space wrap>
            <InputNumber min={1} max={Number.MAX_SAFE_INTEGER} precision={0} value={retryId} onChange={setRetryId} placeholder="Mã tiếp nhận" />
            <Popconfirm title="Đã kiểm tra và xử lý nguyên nhân lỗi?" description="Chỉ yêu cầu thử lại dữ liệu gốc, không thay đổi kết quả hoặc xác nhận PLC."
              onConfirm={async () => { if (!retryId) return; try { await retryAs200Inbox(retryId); messageApi.info("Đã gửi yêu cầu thử lại. Vui lòng theo dõi trạng thái xử lý."); } catch { messageApi.error("Không gửi được yêu cầu thử lại."); } }}>
              <Button disabled={!retryId}>Yêu cầu thử lại</Button>
            </Popconfirm>
          </Space>} />
      )}

      {(status?.volatileCount ?? 0) > 0 && (pendingStorageFailure || journalWriteFailure) && (
        <Alert
          type="error"
          showIcon
          style={{ marginBottom: 16 }}
          message={`${status?.volatileCount} kết quả đo chưa lưu được vào PostgreSQL.`}
          description={journalWriteFailure
            ? "Chưa xác nhận được dữ liệu chờ đã lưu an toàn vào Journal. Vui lòng liên hệ kỹ thuật; không đóng ứng dụng."
            : "Dữ liệu đang chờ trong Journal trên máy, chưa được xác nhận lưu vào PostgreSQL. Vui lòng kiểm tra nguyên nhân trước khi tiếp tục."}
        />
      )}

      {(visibleWarnings.length > 0 || (status?.pauseReason && status.pauseReason !== "RESET")) && (
        <Card className="section-card add-measurement-warnings">
          <Space orientation="vertical" style={{ width: "100%" }}>
            {visibleWarnings.map((warning) => (
              <Alert
                key={`${warning.severity}:${warning.message}`}
                type={warning.severity}
                showIcon
                message={warning.message}
              />
            ))}

            {(status?.pauseReason === "OVERFLOW" ||
              status?.pauseReason === "QUEUE_FULL") && (
              <Popconfirm
                title="Đã kiểm tra nguy cơ mất kết quả đo?"
                description="Tiếp tục không khôi phục được các kết quả đo đã bị ghi đè. Cảnh báo vẫn được lưu."
                okText="Đã kiểm tra, tiếp tục"
                cancelText="Hủy"
                onConfirm={() => void acknowledge()}
              >
                <Button>
                  Tiếp tục sau khi kiểm tra dữ liệu
                </Button>
              </Popconfirm>
            )}

            {status?.pauseReason && status.pauseReason !== "RESET" &&
              status.pauseReason !== "OVERFLOW" &&
              status.pauseReason !== "QUEUE_FULL" && (
                <p>
                  {status.pauseReason === "DIAGNOSTIC_FULL"
                    ? "Vui lòng liên hệ kỹ thuật để kiểm tra vùng lưu chẩn đoán. Giữ nguyên dữ liệu chờ."
                    : "Vui lòng liên hệ kỹ thuật để kiểm tra nguyên nhân và xác nhận phiên kết nối trước khi khởi động lại."}
                </p>
              )}
          </Space>
        </Card>
      )}

      <RecentMeasurements records={recent} detail={activeDetail} loading={monitorLoading} error={monitorError}
        onRefresh={() => setRefreshIndex((value) => value + 1)} />

    </div>
  );
}

export default AddMeasurement;
