import { ApiOutlined, CheckCircleOutlined, SaveOutlined } from "@ant-design/icons";
import { Alert, Button, Card, Empty, Form, Input, InputNumber, Select, Tag } from "antd";
import { useEffect, useRef, useState } from "react";
import PageHeader from "../../components/PageHeader";
import { usePlcMonitor } from "../../hooks/usePlcMonitor";
import { formatReportTime } from "../../utils/reportTime";
import { getPlcConnectionSettings, getPlcRegisterUiEnabled, isPlcIp, plcConnectionError, testPlcRuntimeSettings, savePlcRuntimeSettings, stopPlcCollection, type PlcConnectionSettings, type PlcEndpoint, type PlcRuntimeParameters } from "../../services/plcConnectionService";
import "./ConnectionSettings.css";
import PlcRegisterCard from "./PlcRegisterCard";
import { connectVerifiedPlc, type PlcConnectionOutcome, type PlcInspectionReport } from "../../services/plcConnectionService";

/** Giá trị điền sẵn khi chưa có cấu hình đã lưu; người dùng vẫn phải kiểm tra và bấm lưu. */
const DEFAULT_ENDPOINT: PlcEndpoint = { ip: "192.168.1.5", port: 502, unitId: 1 };

const editableParameters = ["controlStart", "modbusAddressOffset", "pollIntervalMs", "timeoutMs", "retryIntervalMs", "wordOrder", "heartbeatTimeoutMs"] as const;
type EditableParameters = Pick<PlcRuntimeParameters, typeof editableParameters[number]>;

export default function ConnectionSettings() {
  const [form] = Form.useForm<PlcEndpoint & EditableParameters>();
  const formValues = Form.useWatch([], form);
  const [technicalMode, setTechnicalMode] = useState(false);
  const [needsTechnical, setNeedsTechnical] = useState(false);
  const [settings, setSettings] = useState<PlcConnectionSettings>();
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<"test" | "save" | "connect" | "stop" | null>(null);
  const [dirty, setDirty] = useState(false);
  const [registerBusy, setRegisterBusy] = useState(false);
  const [registerUiEnabled, setRegisterUiEnabled] = useState(false);
  const [error, setError] = useState("");
  const [connectionTestError, setConnectionTestError] = useState("");
  const [notice, setNotice] = useState("");
  const [inspection, setInspection] = useState<{ endpoint: PlcEndpoint; report: PlcInspectionReport; reason?: string }>();
  const [hiddenLogEntries, setHiddenLogEntries] = useState<Set<string>>(() => new Set());
  const { status: worker, settings: liveSettings, error: monitorError, refresh: refreshWorker } = usePlcMonitor();
  const inFlight = useRef(false);
  const mounted = useRef(true);
  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      try {
        const enabled = await getPlcRegisterUiEnabled();
        if (!disposed) {
          setRegisterUiEnabled(enabled);
          if (!enabled) setRegisterBusy(false);
        }
      } catch {
        if (!disposed) { setRegisterUiEnabled(false); setRegisterBusy(false); }
      } finally {
        if (!disposed) timer = setTimeout(() => void refresh(), 2000);
      }
    };
    void refresh();
    return () => { disposed = true; clearTimeout(timer); };
  }, []);
  useEffect(() => {
    mounted.current = true;
    void getPlcConnectionSettings().then((value) => {
      if (!mounted.current) return;
      setSettings(value); form.setFieldsValue({ ...value.endpoint, ...value.parameters });
    }).catch((e: unknown) => { if (mounted.current) setError(plcConnectionError(e, "Không thể đọc thông tin kết nối. Vui lòng thử lại hoặc liên hệ kỹ thuật.")); })
      .finally(() => { if (mounted.current) setLoading(false); });
    return () => { mounted.current = false; };
  }, [form]);
  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => { if (dirty) { event.preventDefault(); event.returnValue = ""; } };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);
  const changed = () => {
    const values = form.getFieldsValue();
    setDirty(!!settings && (!settings.endpoint || values.ip !== settings.endpoint.ip || values.port !== settings.endpoint.port || values.unitId !== settings.endpoint.unitId
      || editableParameters.some(key => values[key] !== settings.parameters[key])));
    setInspection(undefined); setConnectionTestError(""); setError(""); setNotice(""); setNeedsTechnical(false);
  };
  const showConnectionOutcome = (outcome: PlcConnectionOutcome, endpoint: PlcEndpoint) => {
    if (outcome.collectionStarted) {
      setInspection(undefined); setNeedsTechnical(false);
      setNotice("Đã đối chiếu hồ sơ và khởi động thu dữ liệu.");
    } else if (outcome.inspection) {
      setInspection({ endpoint, report: outcome.inspection, reason: outcome.blockReason ?? undefined });
      setNeedsTechnical(false); setNotice("");
    }
  };
  const act = async (action: "test" | "save" | "connect" | "stop") => {
    if (inFlight.current || !settings || registerBusy) return;
    inFlight.current = true;
    try {
      setBusy(action); setError(""); setConnectionTestError(""); setNotice(""); setInspection(undefined);
      if (action === "stop") {
        await stopPlcCollection(); await refreshWorker();
        setSettings(await getPlcConnectionSettings()); setNotice("Đã ngắt kết nối và dừng thu; dữ liệu chờ được giữ nguyên.");
        return;
      }
      const values = await form.validateFields(technicalMode ? undefined : ["ip", "port", "unitId"]);
      const { ip, port, unitId } = values;
      // Hidden and profile-owned values are preserved from the saved backend configuration.
      const parameters: PlcRuntimeParameters = {
        ...settings.parameters,
        controlStart: technicalMode ? values.controlStart : settings.parameters.controlStart,
        modbusAddressOffset: technicalMode ? values.modbusAddressOffset : settings.parameters.modbusAddressOffset,
        pollIntervalMs: technicalMode ? values.pollIntervalMs : settings.parameters.pollIntervalMs,
        timeoutMs: technicalMode ? values.timeoutMs : settings.parameters.timeoutMs,
        retryIntervalMs: technicalMode ? values.retryIntervalMs : settings.parameters.retryIntervalMs,
        wordOrder: technicalMode ? values.wordOrder : settings.parameters.wordOrder,
        heartbeatTimeoutMs: technicalMode ? values.heartbeatTimeoutMs : settings.parameters.heartbeatTimeoutMs,
      };
      const endpoint = { ip, port, unitId };
      const runtime = { endpoint, parameters };
      if (action === "test") {
        setInspection(undefined);
        const report = await testPlcRuntimeSettings(runtime);
        if (mounted.current) { setInspection({ endpoint, report }); setNeedsTechnical(false); }
        return;
      }
      if (action === "save") {
        const saved = await savePlcRuntimeSettings(runtime, settings.revision);
        if (mounted.current) { setSettings(saved); form.setFieldsValue({ ...saved.endpoint, ...saved.parameters }); setDirty(false); }
      } else {
        // Technical edits are saved as an inactive draft before the same guarded connect.
        const savedDraft = await savePlcRuntimeSettings(runtime, settings.revision);
        setSettings(savedDraft);
        const outcome = await connectVerifiedPlc(endpoint, savedDraft.revision);
        showConnectionOutcome(outcome, endpoint);
        await refreshWorker();
        const saved = await getPlcConnectionSettings();
        if (mounted.current) {
          setSettings(saved); form.setFieldsValue({ ...saved.endpoint, ...saved.parameters }); setDirty(false);
          if (outcome.collectionStarted && saved.parameters.readOnly) setNotice("Đã kết nối và bắt đầu thu dữ liệu. ACK đang tắt.");
        }
      }
      if (mounted.current && action === "save") setNotice("Đã lưu cấu hình chờ. Nhấn Kết nối để đối chiếu và áp dụng; nguồn đang thu không thay đổi.");
    } catch (e) {
      if (mounted.current && !(e && typeof e === "object" && "errorFields" in e)) {
        const current = await getPlcConnectionSettings().catch(() => undefined);
        if (current && mounted.current) { setSettings(current); form.setFieldsValue({ ...current.endpoint, ...current.parameters }); setDirty(false); }
        setNotice("");
        setNeedsTechnical(typeof e === "string" && /xác minh|RESET|Journal|ACK|Session|đối chiếu/i.test(e));

        const message = plcConnectionError(e, action === "test" ? "Không thể kết nối với PLC. Kiểm tra địa chỉ và thông số vùng nhớ." : "Không thể lưu hoặc áp dụng cấu hình PLC.");
        if (action === "test") setConnectionTestError(message);
        else setError(message);
      }
    } finally { inFlight.current = false; if (mounted.current) setBusy(null); }
  };
  const connectQc = async () => {
    if (inFlight.current || !settings || registerBusy) return;
    inFlight.current = true;
    try {
      const values = await form.validateFields(["ip", "port", "unitId"]);
      setBusy("connect"); setError(""); setConnectionTestError(""); setNotice(""); setNeedsTechnical(false);
      setInspection(undefined);
      const endpoint = { ip: values.ip, port: values.port, unitId: values.unitId };
      const outcome = await connectVerifiedPlc(endpoint, settings.revision);
      showConnectionOutcome(outcome, endpoint);
      await refreshWorker();
      const saved = await getPlcConnectionSettings();
      if (mounted.current) {
        setSettings(saved); form.setFieldsValue({ ...saved.endpoint, ...saved.parameters }); setDirty(false);
        if (outcome.collectionStarted && saved.parameters.readOnly) setNotice("Đã kết nối và bắt đầu thu dữ liệu. ACK đang tắt.");
      }
    } catch (reason) {
      if (mounted.current && !(reason && typeof reason === "object" && "errorFields" in reason)) {
        setNeedsTechnical(typeof reason === "string" && /chưa được xác minh|RESET_CONFIRMATION_REQUIRED|reset|Journal|ACK|Session|phiên|đối chiếu/i.test(reason));
        setError(plcConnectionError(reason, "Không thể kết nối PLC. Vui lòng kiểm tra endpoint hoặc liên hệ kỹ thuật."));
        await refreshWorker();
        const saved = await getPlcConnectionSettings().catch(() => undefined);
        if (mounted.current && saved) { setSettings(saved); form.setFieldsValue({ ...saved.endpoint, ...saved.parameters }); setDirty(false); }
      }
    } finally { inFlight.current = false; if (mounted.current) setBusy(null); }
  };
  const canEditConnection = !!settings && worker?.state !== "STOPPING" && !registerBusy;
  const activeSettings = liveSettings ?? settings;
  const applied = activeSettings?.activeParameters;
  const memory = applied ?? settings?.parameters;
  const memoryValid = memory?.layoutProfile === "as200-published-v1" && memory.bufferCount === 2 && memory.recordSize === 240
    && memory.controlStart + 20 + memory.bufferCount * memory.recordSize <= 65536;
  const tcpLabel = busy === "connect" ? "Đang kết nối"
    : inspection ? inspection.report.modbusReadSucceeded ? "Đã kết nối để kiểm tra" : inspection.report.tcpConnected ? "TCP đã kết nối · Modbus chưa hợp lệ" : "Không kết nối được TCP"
    : needsTechnical ? "Cần kỹ thuật xác minh"
    : error || monitorError || worker?.state === "ERROR" || worker?.pauseReason ? "Lỗi"
    : monitorError || !worker ? "Chưa xác minh"
    : worker.connection === "CONNECTED" ? "Đã kết nối"
    : worker.connection === "DISCONNECTED" ? "Mất kết nối"
    : worker.connection === "CONNECTING" ? "Đang kết nối" : "Chưa kết nối";
  const ackLabels: Record<string, string> = {
    READ_ONLY_PROTOCOL_UNCONFIRMED: "Chỉ đọc · giao thức chưa xác nhận", DISABLED: "Đã tắt",
    WAITING: "Đang chờ xác minh", PENDING: "Có yêu cầu đang chờ", ACKNOWLEDGED: "Đã được backend xác minh",
    RECONCILIATION_REQUIRED: "Chưa xác định · cần đối chiếu, không gửi lại", FAILED: "Có lỗi · chưa xác nhận hoàn tất",
  };
  const workerHasError = !!worker && (!!worker.pauseReason || ["ERROR", "FAILED", "PAUSED", "SESSION_UNCONFIRMED"].includes(worker.state));
  const currentError = monitorError || (workerHasError ? plcConnectionError(worker?.message, "Worker có lỗi, cần kiểm tra trước khi tiếp tục.") : "");
  const hasLiveSnapshot = !!worker?.enabled && worker.connection === "CONNECTED"
    && !monitorError && !workerHasError && worker.plcHealth !== "INVALID_DATA";
  const snapshot = hasLiveSnapshot ? worker.controlSnapshot : undefined;
  const kpis = [
    ["Heartbeat PLC", snapshot?.heartbeat],
    ["Độ trễ đọc", hasLiveSnapshot && worker.lastReadMs != null ? `${worker.lastReadMs} ms` : null],
    ["TotalCount", snapshot?.total_count],
    ["RecordSeq", hasLiveSnapshot ? worker.publishedRecordSeq : null],
    ["DataReady", snapshot?.data_ready],
    ["PC_ReadDone", snapshot?.pc_read_done],
    ["PC_ReadCount", snapshot?.pc_read_count],
    ["Lỗi đọc hôm nay", worker?.readTelemetry?.readErrors],
    ["Mất kết nối hôm nay", worker?.readTelemetry?.disconnects],
  ] as const;
  const isConnected = worker?.connection === "CONNECTED";
  const connectedEndpoint = activeSettings?.activeEndpoint ?? settings?.endpoint;
  const endpointEdited = !!connectedEndpoint && !!formValues
    && (formValues.ip !== connectedEndpoint.ip || formValues.port !== connectedEndpoint.port || formValues.unitId !== connectedEndpoint.unitId);
  const ackUncertain = ["FAILED", "RECONCILIATION_REQUIRED"].includes(worker?.ackStatus ?? "");
  const logKey = (event: { at: string; message: string }) => JSON.stringify([event.at, event.message]);
  const visibleEvents = (worker?.connectionEvents ?? []).filter(event => !hiddenLogEntries.has(logKey(event)));

  return <div className="plc-settings-page">
    <PageHeader eyebrow="HỆ THỐNG" title="4 · Kết nối PLC" description="Cài đặt kết nối và theo dõi trạng thái tiếp nhận dữ liệu từ PLC." />
    <div className="plc-settings-grid">
      <Card className="plc-settings-card" title={technicalMode ? "Thiết lập kỹ thuật" : "Cài đặt kết nối"} loading={loading}
        extra={<Button size="small" disabled={!!busy || registerBusy} onClick={() => {
          setTechnicalMode(value => !value); setError(""); setNotice(""); setNeedsTechnical(false); setInspection(undefined);
          if (settings) { form.setFieldsValue({ ...settings.endpoint, ...settings.parameters }); setDirty(false); }
        }}>{technicalMode ? "Về kết nối QC" : "Thiết lập kỹ thuật"}</Button>}>
        <Form form={form} layout="vertical" initialValues={DEFAULT_ENDPOINT} disabled={!!busy || loading || registerBusy} onValuesChange={changed} requiredMark={false}>
          {technicalMode && <>
          <h3 className="plc-config-group">PLC</h3>
          <div className="plc-settings-fields">
            <Form.Item label="Loại PLC"><Select value="AS200" options={[{ value: "AS200", label: "Delta AS200 / AS228" }]} /></Form.Item>
            <Form.Item label="Giao thức"><Select value="Modbus TCP" options={[{ value: "Modbus TCP", label: "Modbus TCP" }]} /></Form.Item>
          </div>
          </>}
          <Form.Item name="ip" label="Địa chỉ IP" rules={[{ required: true, message: "Vui lòng nhập địa chỉ IP." }, { validator: (_, value: string) => !value || isPlcIp(value) ? Promise.resolve() : Promise.reject(new Error("Địa chỉ IP chưa hợp lệ. Vui lòng kiểm tra lại.")) }]}>
            <Input placeholder="Ví dụ: 192.168.1.5" autoComplete="off" disabled={!canEditConnection} />
          </Form.Item>
          <div className="plc-settings-fields">
            <Form.Item name="port" label="Cổng kết nối" rules={[{ required: true, message: "Vui lòng nhập cổng kết nối." }, { type: "integer", min: 1, max: 65535, message: "Cổng kết nối phải từ 1 đến 65535." }]}>
              <InputNumber min={1} max={65535} precision={0} disabled={!canEditConnection} />
            </Form.Item>
            <Form.Item name="unitId" label="Unit ID" rules={[{ required: true, message: "Vui lòng nhập Unit ID." }, { type: "integer", min: 1, max: 247, message: "Unit ID phải từ 1 đến 247." }]}>
              <InputNumber min={1} max={247} precision={0} disabled={!canEditConnection} />
            </Form.Item>
          </div>
          {technicalMode && <>
          <h3 className="plc-config-group">Vùng nhớ giao tiếp</h3>
          <div className="plc-settings-fields">
            {([
              ["controlStart", "Thanh ghi bắt đầu (D)", 0, 65535],
              ["modbusAddressOffset", "Lệch địa chỉ Modbus", -65535, 65535],
            ] as const).map(([name, label, min, max]) => <Form.Item key={name} name={name} label={label}
              rules={[{ required: true, message: "Vui lòng nhập giá trị." }, { type: "integer", min, max, message: `Giá trị phải từ ${min} đến ${max}.` }]}>
              <InputNumber min={min} max={max} precision={0} />
            </Form.Item>)}
            <Form.Item label="Số buffer" htmlFor="bufferCount">
              <Input id="bufferCount" className="plc-profile-value" readOnly disabled={false} value={settings?.parameters.bufferCount ?? ""} placeholder="—" />
            </Form.Item>
            <Form.Item label="Kích thước bản ghi (WORD)" htmlFor="recordSize">
              <Input id="recordSize" className="plc-profile-value" readOnly disabled={false} value={settings?.parameters.recordSize ?? ""} placeholder="—" />
            </Form.Item>
            {([
              ["pollIntervalMs", "Chu kỳ đọc (ms)", 50, 60000],
              ["timeoutMs", "Timeout kết nối / đọc (ms)", 100, 30000],
              ["retryIntervalMs", "Chu kỳ retry (ms)", 100, 60000],
            ] as const).map(([name, label, min, max]) => <Form.Item key={name} name={name} label={label}
              rules={[{ required: true, message: "Vui lòng nhập giá trị." }, { type: "integer", min, max, message: `Giá trị phải từ ${min} đến ${max}.` }]}>
              <InputNumber min={min} max={max} precision={0} />
            </Form.Item>)}
            <Form.Item name="wordOrder" label="Thứ tự word REAL/DINT" rules={[{ required: true }]}>
              <Select options={[{ value: "LOW", label: "Word thấp trước" }, { value: "HIGH", label: "Word cao trước" }]} />
            </Form.Item>
            <Form.Item name="heartbeatTimeoutMs" label="Timeout heartbeat (s)"
              getValueProps={(value: number | null) => ({ value: value == null ? null : value / 1000 })}
              normalize={(value: number | null) => value == null ? null : value * 1000}
              rules={[{ required: true, message: "Cần ngưỡng heartbeat đã xác nhận." }, { type: "integer", min: 2000, max: 3600000, message: "Heartbeat phải 2–3600 giây." }]}>
              <InputNumber min={2} max={3600} precision={0} />
            </Form.Item>
          </div>
          {settings?.restartRequired && <Tag color="gold">Đã lưu · chờ kết nối</Tag>}
          <div className="plc-settings-actions">
            <Button type="primary" icon={<SaveOutlined />} loading={busy === "save"} disabled={!!busy || !canEditConnection} onClick={() => void act("save")}>Lưu cài đặt</Button>
            <Button className="plc-button-outline" icon={<ApiOutlined />} loading={busy === "test"} disabled={!!busy || !settings || registerBusy} onClick={() => void act("test")}>Kiểm tra kết nối</Button>
            <Button className={isConnected && !endpointEdited ? undefined : "plc-button-connect"} danger={isConnected && !endpointEdited}
              loading={busy === "connect" || busy === "stop"} disabled={!!busy || !canEditConnection || !worker || !!monitorError}
              onClick={() => void act(isConnected && !endpointEdited ? "stop" : "connect")}>{isConnected && !endpointEdited ? "Ngắt kết nối" : "Kết nối"}</Button>
          </div>
          </>}
          {!technicalMode && <div className="plc-settings-actions">
            <Button className="plc-button-outline" icon={<ApiOutlined />} loading={busy === "test"} disabled={!!busy || !settings || registerBusy} onClick={() => void act("test")}>Kiểm tra kết nối</Button>
            <Button className={isConnected && !endpointEdited ? undefined : "plc-button-connect"} danger={isConnected && !endpointEdited}
              loading={busy === "connect" || busy === "stop"}
              disabled={!!busy || !canEditConnection || !worker || !!monitorError}
              onClick={() => isConnected && !endpointEdited ? void act("stop") : void connectQc()}>{isConnected && !endpointEdited ? "Ngắt kết nối" : "Kết nối"}</Button>
          </div>}
        </Form>
        <div className="plc-settings-feedback" aria-live="polite">
          {error && <Alert type="error" showIcon title={error} />}
          {inspection && <p className={`plc-connection-test-result ${inspection.report.tcpConnected && inspection.report.modbusReadSucceeded && inspection.report.as200Valid ? "success" : "error"}`}>
            {inspection.report.tcpConnected && inspection.report.modbusReadSucceeded && inspection.report.as200Valid
              ? "✓ Kiểm tra kết nối thành công."
              : inspection.report.message}
          </p>}
          {connectionTestError && <p className="plc-connection-test-result error">{connectionTestError}</p>}
          {notice && <p className="plc-settings-saved"><CheckCircleOutlined /> {notice}</p>}
        </div>
      </Card>
      <div className="plc-settings-right">
        <Card className="plc-settings-card plc-status-card" title="Trạng thái kết nối"
          extra={<span className={`page-status-pill ${inspection ? inspection.report.modbusReadSucceeded ? "page-status-unverified" : "page-status-error" : needsTechnical || busy === "connect" ? "page-status-unverified" : error || monitorError || workerHasError || worker?.connection === "DISCONNECTED" ? "page-status-error" : isConnected ? "page-status-connected" : "page-status-unverified"}`}>
            <i aria-hidden="true" />{tcpLabel}
          </span>}>
          {currentError && <Alert type="error" showIcon title={currentError} />}
          {ackUncertain && <Alert type="error" showIcon title={worker ? ackLabels[worker.ackStatus] : "ACK cần đối chiếu"}
            description="Giữ dữ liệu và yêu cầu đang chờ; không tự gửi lại ACK." />}
          {!currentError && worker?.lastError && worker.state === "ERROR" && <Alert type="error" showIcon title={plcConnectionError(worker.lastError, "Lỗi đọc/kết nối PLC")} />}
          <div className="plc-mini-kpis" aria-live="polite">{kpis.map(([label, value]) =>
            <div key={label}><span>{label}</span><strong>{value ?? "—"}</strong></div>)}</div>
        </Card>
        <Card className="plc-settings-card" title="Sơ đồ vùng nhớ"
          extra={memoryValid && memory ? <Tag>{applied ? "Đang dùng" : "Đã lưu"} · Offset {memory.modbusAddressOffset}</Tag> : undefined}>
          {memoryValid && memory ? <div className="plc-memory-map">
            <div><b>D{memory.controlStart}–D{memory.controlStart + 19}</b><span>Vùng điều khiển</span></div>
            {Array.from({ length: memory.bufferCount }, (_, index) => {
              const start = memory.controlStart + 20 + index * memory.recordSize;
              return <div key={index}><b>D{start}–D{start + memory.recordSize - 1}</b><span>Bản ghi {index + 1} · {memory.recordSize} WORD</span></div>;
            })}
          </div> : <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="Chưa có vùng nhớ hợp lệ" />}
        </Card>
        <Card className="plc-settings-card plc-connection-events" title="Nhật ký kết nối"
          extra={<Button size="small" disabled={!visibleEvents.length} onClick={() => setHiddenLogEntries(previous =>
            new Set([...previous, ...(worker?.connectionEvents ?? []).map(logKey)]))}>Xóa</Button>}>
          <div className="plc-event-log" role="log">
            {visibleEvents.length ? visibleEvents.map((event, index) => {
              const tone = /lỗi|không thể|thất bại|mất kết nối|timeout|failed|error/i.test(event.message) ? "error"
                : /chờ|đối chiếu|cảnh báo|reset|ACK/i.test(event.message) ? "warning" : "info";
              return <div key={`${logKey(event)}-${index}`} className={`plc-log-${tone}`}><time>{formatReportTime(event.at)}</time><span>{plcConnectionError(event.message, "Lỗi kết nối/đọc PLC")}</span></div>;
            }) : <span className="plc-log-empty">Chưa có sự kiện kết nối.</span>}
          </div>
        </Card>
      </div>
    </div>
    {registerUiEnabled && <PlcRegisterCard form={form as unknown as import("antd").FormInstance<PlcEndpoint>} worker={worker} disabled={!!busy || loading} onBusyChange={setRegisterBusy}
      onStopped={async () => { await refreshWorker(); }} />}
  </div>;
}
