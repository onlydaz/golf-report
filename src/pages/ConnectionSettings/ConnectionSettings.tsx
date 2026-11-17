import { ApiOutlined, CheckCircleOutlined, SaveOutlined } from "@ant-design/icons";
import { Alert, Button, Card, Form, Input, InputNumber, Popconfirm } from "antd";
import { useEffect, useRef, useState } from "react";
import PageHeader from "../../components/PageHeader";
import { getPlcDisplayStatus, usePlcMonitor } from "../../hooks/usePlcMonitor";
import { formatReportTime } from "../../utils/reportTime";
import { clearPlcConnectionLog, connectPlcCollection, isPlcIp, plcConnectionError, savePlcConnectionOptions, stopPlcCollection, testPlcConnection,
  type PlcEndpoint, type PlcInspectionReport } from "../../services/plcConnectionService";
import "./ConnectionSettings.css";

export default function ConnectionSettings() {
  const [form] = Form.useForm<PlcEndpoint>();
  const { status: worker, settings, error: monitorError, refresh } = usePlcMonitor();
  const [busy, setBusy] = useState<"test" | "connect" | "stop" | "save" | null>(null);
  const [technicalMode, setTechnicalMode] = useState(false);
  const [pollInterval, setPollInterval] = useState<number | null>(null);
  const [localIp, setLocalIp] = useState("");
  const savedLocalIp = settings?.plcLocalIp ?? "";
  useEffect(() => { setLocalIp(savedLocalIp); }, [savedLocalIp]);
  const localIpValid = !localIp.trim() || isPlcIp(localIp.trim());
  const localIpChanged = localIp.trim() !== savedLocalIp;
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [inspection, setInspection] = useState<PlcInspectionReport>();
  const [clearingLog, setClearingLog] = useState(false);
  const [logError, setLogError] = useState("");
  const [clock, setClock] = useState(Date.now);
  const connectionRunning = !!worker?.enabled && worker.connection === "CONNECTED"
    && worker.state !== "STOPPING" && !!worker.connectedAt && !worker.connectionEndedAt;
  useEffect(() => {
    if (!connectionRunning) return;
    setClock(Date.now());
    const timer = setInterval(() => setClock(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [connectionRunning, worker?.connectedAt]);
  const inFlight = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { if (settings) form.setFieldsValue(settings.endpoint); }, [form, settings]);
  const savedPollInterval = settings?.parameters.pollIntervalMs;
  useEffect(() => {
    setPollInterval(savedPollInterval ?? null);
  }, [savedPollInterval]);
  const pollIntervalValid = pollInterval !== null && Number.isInteger(pollInterval) && pollInterval >= 50 && pollInterval <= 60000;
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  const act = async (action: "test" | "connect" | "stop" | "save") => {
    if (inFlight.current) return;
    if (action === "save" && !pollIntervalValid) {
      setError("Chu kỳ đọc phải là số nguyên từ 50 đến 60000 ms.");
      return;
    }
    if (action === "save" && !localIpValid) {
      setError("IP nguồn PLC không hợp lệ. Nhập IP Ethernet hoặc để trống để hệ điều hành tự chọn.");
      return;
    }
    inFlight.current = true;
    setBusy(action); setError(""); setNotice(""); setInspection(undefined);
    try {
      if (action === "save") {
        const saved = await savePlcConnectionOptions(pollInterval!, localIp.trim());
        if (mounted.current) {
          setPollInterval(saved.parameters.pollIntervalMs);
          setLocalIp(saved.plcLocalIp ?? "");
          setNotice("Đã lưu cài đặt. Giá trị mới áp dụng khi bắt đầu thu dữ liệu lần tiếp theo.");
        }
        // Saving already succeeded; a failed status refresh must not report it as a save failure.
        await refresh().catch(() => undefined);
      } else if (action === "test") {
        const report = await testPlcConnection();
        if (mounted.current) setInspection(report);
      } else {
        if (action === "stop") await stopPlcCollection();
        else await connectPlcCollection();
        await refresh();
        if (mounted.current) setNotice(action === "stop" ? "Đã dừng thu; dữ liệu chờ được giữ nguyên."
          : "Đã khởi động bộ thu. Theo dõi trạng thái PLC và ACK bên phải.");
      }
    } catch (reason) {
      if (mounted.current) setError(plcConnectionError(reason, action === "save" ? "Không thể lưu chu kỳ đọc." : "Không thể thực hiện thao tác kết nối PLC."));
      await refresh().catch(() => undefined);
    } finally { inFlight.current = false; if (mounted.current) setBusy(null); }
  };
  const display = getPlcDisplayStatus(worker, monitorError);
  const workerHasError = !!worker && (!!worker.pauseReason || ["ERROR", "FAILED", "PAUSED"].includes(worker.state));
  const currentError = monitorError || (workerHasError ? plcConnectionError(worker?.message, "Bộ thu có lỗi; cần đối chiếu.") : "");
  const hasLiveSnapshot = !!worker?.enabled && worker.connection === "CONNECTED" && !monitorError && worker.plcHealth !== "INVALID_DATA";
  const snapshot = hasLiveSnapshot ? worker.controlSnapshot : undefined;
  const memory = settings?.parameters;
  const memoryFields = [
    ["controlStart", "Thanh ghi bắt đầu (D)", memory?.controlStart],
    ["modbusAddressOffset", "Lệch địa chỉ Modbus", memory?.modbusAddressOffset],
    ["recordSize", "Kích thước bản ghi (WORD)", memory?.recordSize],
    ["pollIntervalMs", "Chu kỳ đọc (ms)", null],
    ["timeoutMs", "Timeout kết nối / đọc (ms)", memory?.timeoutMs],
    ["retryIntervalMs", "Chu kỳ retry (ms)", memory?.retryIntervalMs],
    ["wordOrder", "Thứ tự word REAL/DINT", memory?.wordOrder === "LOW" ? "Word thấp trước" : memory?.wordOrder === "HIGH" ? "Word cao trước" : null],
    ["heartbeatTimeoutMs", "Timeout heartbeat (s)", memory?.heartbeatTimeoutMs != null ? memory.heartbeatTimeoutMs / 1000 : null],
  ] as const;
  const kpis = [
    ["Thời gian kết nối", worker?.connectedAt ? formatReportTime(worker.connectedAt)
      : (() => { const event = [...(worker?.connectionEvents ?? [])].reverse().find(item => item.kind === "CONNECTED" || item.kind === "RECONNECTED"); return event ? formatReportTime(event.at) : null; })()],
    ["Thời gian đã kết nối", (() => {
      if (!worker?.connectedAt) return null;
      const end = worker.connectionEndedAt ? Date.parse(worker.connectionEndedAt) : connectionRunning ? clock : NaN;
      if (!Number.isFinite(end)) return null;
      const seconds = Math.max(0, Math.floor((end - Date.parse(worker.connectedAt)) / 1000));
      const pad = (value: number) => String(value).padStart(2, "0");
      return `${pad(Math.floor(seconds / 3600))} giờ ${pad(Math.floor(seconds / 60) % 60)} phút ${pad(seconds % 60)} giây`;
    })()],
    ["Heartbeat PLC", snapshot?.heartbeat], ["Độ trễ đọc", hasLiveSnapshot && worker.lastReadMs != null ? `${worker.lastReadMs} ms` : null],
    ["DataReady", snapshot?.data_ready], ["PC_ReadDone", snapshot?.pc_read_done], ["PC_ReadCount", snapshot?.pc_read_count],
    ["Lỗi đọc hôm nay", worker?.readTelemetry?.readErrors], ["Mất kết nối hôm nay", worker?.readTelemetry?.disconnects],
  ] as const;
  const ackUncertain = ["FAILED", "RECONCILIATION_REQUIRED"].includes(worker?.ackStatus ?? "");
  const logKey = (event: { at: string; message: string }) => JSON.stringify([event.at, event.message]);
  const events = [...(worker?.connectionEvents ?? [])].reverse()
    .sort((a, b) => Date.parse(b.at) - Date.parse(a.at));
  const clearLog = async () => {
    if (clearingLog) return;
    setClearingLog(true); setLogError("");
    try {
      await clearPlcConnectionLog();
      await refresh();
    } catch (reason) {
      if (mounted.current) setLogError(plcConnectionError(reason, "Không thể xóa hoặc tải lại nhật ký kết nối."));
    } finally { if (mounted.current) setClearingLog(false); }
  };
  return <div className="plc-settings-page">
    <PageHeader eyebrow="HỆ THỐNG" title="4 · Kết nối PLC" description="Theo dõi kết nối và thu dữ liệu từ PLC." />
    <div className="plc-settings-grid">
      <Card className="plc-settings-card" title={technicalMode ? "Thiết lập kỹ thuật" : "Cài đặt kết nối"}
        extra={<Button size="small" disabled={!!busy} onClick={() => setTechnicalMode(value => !value)}>
          {technicalMode ? "Về kết nối QC" : "Thiết lập kỹ thuật"}
        </Button>}>
        <Form form={form} layout="vertical" requiredMark={false}>
          {technicalMode && <>
            <h3 className="plc-config-group">PLC</h3>
            <div className="plc-settings-fields">
              <Form.Item label="Loại PLC"><Input readOnly value={memory?.plcType ?? ""} placeholder="—" /></Form.Item>
              <Form.Item label="Giao thức"><Input readOnly value={memory?.protocol ?? ""} placeholder="—" /></Form.Item>
            </div>
          </>}
          <Form.Item name="ip" label="Địa chỉ IP"><Input readOnly placeholder="—" /></Form.Item>
          <div className="plc-settings-fields">
            <Form.Item name="port" label="Cổng kết nối"><Input readOnly placeholder="—" /></Form.Item>
            <Form.Item name="unitId" label="Unit ID"><Input readOnly placeholder="—" /></Form.Item>
          </div>
          {technicalMode && <>
            <Form.Item label="IP nguồn kết nối PLC (Ethernet)" validateStatus={!localIpValid ? "error" : undefined}
              help={!localIpValid ? "Nhập địa chỉ IP hợp lệ." : "Để trống: hệ điều hành tự chọn. Chọn IP Ethernet của laptop; cần ngắt kết nối trước khi đổi và lưu. Bind IP nguồn không bảo đảm chọn đúng tuyến mạng Windows."}>
              <Input value={localIp} placeholder="Tự động" disabled={!!busy || !settings || !!worker?.enabled || worker?.state === "STOPPING"}
                onChange={event => { setLocalIp(event.target.value); setNotice(""); setError(""); }} />
            </Form.Item>
            <h3 className="plc-config-group">Vùng nhớ giao tiếp</h3>
            <div className="plc-settings-fields">
              {memoryFields.map(([key, label, value]) => key === "pollIntervalMs"
                ? <Form.Item key={key} label={label} validateStatus={settings && !pollIntervalValid ? "error" : undefined}
                    help={settings && !pollIntervalValid ? "Nhập số nguyên từ 50 đến 60000 ms." : undefined}>
                    <InputNumber<number> value={pollInterval} min={50} max={60000} precision={0}
                      disabled={!!busy || !settings}
                      onChange={value => { setPollInterval(value); setNotice(""); setError(""); }} />
                  </Form.Item>
                : <Form.Item key={key} label={label}><Input readOnly value={value ?? ""} placeholder="—" /></Form.Item>)}
            </div>
          </>}
        </Form>
        <div className="plc-settings-actions">
          {technicalMode && <Button type="primary" icon={<SaveOutlined />} loading={busy === "save"}
            disabled={!!busy || !settings || !pollIntervalValid || !localIpValid || (!localIpChanged && pollInterval === savedPollInterval)}
            onClick={() => void act("save")}>Lưu cài đặt</Button>}
          <Button className="plc-button-outline" icon={<ApiOutlined />} loading={busy === "test"} disabled={!!busy || !settings} onClick={() => void act("test")}>Kiểm tra kết nối</Button>
          <Button className={worker?.enabled ? undefined : "plc-button-connect"} danger={worker?.enabled}
            loading={busy === "connect" || busy === "stop"} disabled={!!busy || (!settings && !worker?.enabled) || worker?.state === "STOPPING"}
            onClick={() => void act(worker?.enabled ? "stop" : "connect")}>{worker?.enabled ? "Ngắt kết nối" : "Kết nối"}</Button>
        </div>
        <div className="plc-settings-feedback" aria-live="polite">
          {error && <Alert type="error" showIcon title={error} />}
          {inspection && <Alert showIcon type={inspection.as200Valid ? "success" : "warning"} title={plcConnectionError(inspection.message, "Chưa thể xác minh kết nối PLC.")} description={inspection.mappingWarning ? plcConnectionError(inspection.mappingWarning, "Cần kỹ thuật kiểm tra cấu hình PLC.") : undefined} />}
          {notice && <p className="plc-settings-saved"><CheckCircleOutlined /> {notice}</p>}
        </div>
      </Card>
      <div className="plc-settings-right">
        <Card className="plc-settings-card plc-status-card" title="Trạng thái kết nối"
          extra={<span className={`page-status-pill ${display.verified ? "page-status-connected" : display.failed ? "page-status-error" : "page-status-unverified"}`}><i aria-hidden="true" />{display.plcLabel}</span>}>
          {currentError && <Alert type="error" showIcon title={currentError} />}
          {ackUncertain && <Alert type="error" showIcon title="ACK chưa được đối chiếu" description="Giữ dữ liệu và yêu cầu đang chờ; không tự gửi lại ACK." />}
          <div className="plc-mini-kpis" aria-live="polite">{kpis.map(([label, value]) => <div key={label}><span>{label}</span><strong>{value ?? "—"}</strong></div>)}</div>
        </Card>
        <Card className="plc-settings-card" title="Nhật ký kết nối PLC" extra={
          <Popconfirm title="Xóa toàn bộ nhật ký kết nối PLC?" description="Chỉ xóa nhật ký kết nối, giữ nguyên dữ liệu đo và lịch sử ACK."
            okText="Xóa nhật ký" cancelText="Hủy" onConfirm={clearLog} okButtonProps={{ danger: true, loading: clearingLog }}>
            <Button size="small" disabled={!events.length || clearingLog} loading={clearingLog}>Xóa</Button>
          </Popconfirm>}>
          {(logError || worker?.connectionLogError) && <Alert type="error" showIcon title={logError || worker?.connectionLogError} />}
          <div className="plc-event-log" role="log">{events.length ? events.map(event => {
            const successful = event.kind === "CONNECTED" || event.kind === "RECONNECTED";
            const failed = event.kind === "DISCONNECTED" || event.kind === "FAILED";
            return <div key={logKey(event)} className={successful ? "plc-log-success" : failed ? "plc-log-error" : "plc-log-neutral"}>
              <span aria-hidden="true">{successful ? "🟢" : failed ? "🔴" : "⚪"} </span>
              <time dateTime={event.at}>{formatReportTime(event.at)}</time><span> — {plcConnectionError(event.message, "Đã cập nhật trạng thái kết nối PLC.")}</span>
            </div>;
          }) : <span className="plc-log-empty">Chưa có sự kiện kết nối.</span>}</div>
        </Card>
      </div>
    </div>
  </div>;
}
