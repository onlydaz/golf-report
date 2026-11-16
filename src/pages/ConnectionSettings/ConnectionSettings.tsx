import { ApiOutlined, CheckCircleOutlined, ReloadOutlined, SaveOutlined } from "@ant-design/icons";
import { Alert, Button, Card, Collapse, Empty, Form, Input, InputNumber, Modal, Tag } from "antd";
import { useEffect, useRef, useState } from "react";
import PageHeader from "../../components/PageHeader";
import { getPlcDisplayStatus, usePlcMonitor } from "../../hooks/usePlcMonitor";
import { formatReportTime } from "../../utils/reportTime";
import { getPlcConnectionSettings, getPlcRegisterUiEnabled, isPlcIp, plcConnectionError, testPlcConnection, connectPlcCollection, updatePlcConnectionSettings, type PlcConnectionSettings, type PlcEndpoint } from "../../services/plcConnectionService";
import "./ConnectionSettings.css";
import PlcRegisterCard from "./PlcRegisterCard";

/** Giá trị điền sẵn khi chưa có cấu hình đã lưu; người dùng vẫn phải kiểm tra và bấm lưu. */
const DEFAULT_ENDPOINT: Pick<PlcEndpoint, "ip" | "port"> = { ip: "192.168.1.5", port: 502 };

export default function ConnectionSettings() {
  const [form] = Form.useForm<PlcEndpoint>();
  const [settings, setSettings] = useState<PlcConnectionSettings>();
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<"test" | "save" | null>(null);
  const [dirty, setDirty] = useState(false);
  const [registerBusy, setRegisterBusy] = useState(false);
  const [registerUiEnabled, setRegisterUiEnabled] = useState(false);
  const [error, setError] = useState("");
  const [errorAction, setErrorAction] = useState<"test" | "save" | null>(null);
  const [notice, setNotice] = useState("");
  const [tested, setTested] = useState<"idle" | "ok" | "failed">("idle");
  const [mappingWarning, setMappingWarning] = useState("");
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
      setSettings(value); if (value.endpoint) form.setFieldsValue(value.endpoint);
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
    setDirty(!!settings && (!settings.endpoint || values.ip !== settings.endpoint.ip || values.port !== settings.endpoint.port || values.unitId !== settings.endpoint.unitId));
    setTested("idle"); setMappingWarning(""); setError(""); setNotice("");
  };
  const connect = async (values: PlcEndpoint, revision: string) => {
    try {
      await connectPlcCollection(values, revision);
    } catch (e) {
      if (e !== "RESET_CONFIRMATION_REQUIRED") throw e;
      const confirmed = await new Promise<boolean>((resolve) => {
        Modal.confirm({
          title: "Xác nhận phiên đo mới",
          content: <>Bộ đếm của máy đo đã được đặt lại. Bạn có muốn bắt đầu phiên đo mới không?<br /><br />Dữ liệu đã lưu trước đó vẫn được giữ nguyên.</>,
          okText: "Xác nhận",
          cancelText: "Hủy",
          onOk: () => resolve(true),
          onCancel: () => resolve(false),
        });
      });
      if (!confirmed) throw "Chưa xác nhận phiên mới sau khi Simulator reset; dữ liệu và Journal cũ được giữ nguyên.";
      await connectPlcCollection(values, revision, true);
    }
  };
  const act = async (action: "test" | "save") => {
    if (inFlight.current || !settings || registerBusy) return;
    if (action === "save" && !canEditConnection) return;
    inFlight.current = true;
    try {
      const values = await form.validateFields();
      setBusy(action); setError(""); setNotice("");
      if (action === "test") {
        setTested("idle"); setMappingWarning("");
        const report = await testPlcConnection(values);
        if (report.mappingWarning) console.warn("PLC mapping warning:", report.mappingWarning);
        if (mounted.current) { setTested("ok"); setMappingWarning(report.mappingWarning ?? ""); }
      } else if (!settings.configured) {
        const saved = await updatePlcConnectionSettings(values, settings.revision);
        await connect(values, saved.revision);
        if (mounted.current) {
          await refreshWorker();
          setSettings(await getPlcConnectionSettings());
          setDirty(false);
          setNotice("Đã lưu cấu hình và bắt đầu thu dữ liệu.");
        }
      } else {
        setNotice("Đang kết nối với máy đo...");
        await connect(values, settings.revision);
        const saved = await getPlcConnectionSettings();
        await refreshWorker();
        if (mounted.current) { setSettings(saved); if (saved.endpoint) form.setFieldsValue(saved.endpoint); setDirty(false); setNotice(""); }
      }
    } catch (e) {
      if (mounted.current && !(e && typeof e === "object" && "errorFields" in e)) {
        console.error("PLC connection operation failed:", e);
        setNotice("");
        setErrorAction(action);
        if (action === "test") setTested("failed");
        setError(plcConnectionError(e, action === "test" ? "Không thể kết nối với PLC. Vui lòng kiểm tra địa chỉ IP và cổng kết nối." : "Không thể lưu và bắt đầu thu dữ liệu."));
      }
    } finally { inFlight.current = false; if (mounted.current) setBusy(null); }
  };
  const canEditConnection = !!settings && worker?.state !== "STOPPING" && !registerBusy;
  const display = getPlcDisplayStatus(worker, monitorError);
  const missing = "Chưa có dữ liệu";
  const activeSettings = liveSettings ?? settings;
  const tcpLabel = monitorError || !worker ? "Chưa xác minh"
    : worker.connection === "CONNECTED" ? "Đã kết nối"
    : worker.connection === "DISCONNECTED" ? "Mất kết nối"
    : worker.connection === "CONNECTING" ? "Đang kết nối" : "Chưa kết nối";
  const healthLabels: Record<string, string> = {
    UNCONFIGURED: "Chưa cấu hình", OBSERVING: "Đang quan sát", RUNNING: "Đang thay đổi",
    STALE: "Tín hiệu đứng yên", DISCONNECTED: "Mất kết nối", INVALID_DATA: "Dữ liệu không hợp lệ",
  };
  const ackLabels: Record<string, string> = {
    READ_ONLY_PROTOCOL_UNCONFIRMED: "Chỉ đọc · giao thức chưa xác nhận", DISABLED: "Đã tắt",
    WAITING: "Đang chờ xác minh", PENDING: "Có yêu cầu đang chờ", ACKNOWLEDGED: "Đã được backend xác minh",
    RECONCILIATION_REQUIRED: "Chưa xác định · cần đối chiếu, không gửi lại", FAILED: "Có lỗi · chưa xác nhận hoàn tất",
  };
  const workerHasError = !!worker && (!!worker.pauseReason || ["ERROR", "FAILED", "PAUSED", "SESSION_UNCONFIRMED"].includes(worker.state));
  const currentError = monitorError || (workerHasError ? plcConnectionError(worker?.message, "Worker có lỗi, cần kiểm tra trước khi tiếp tục.") : "");
  const endpointText = (endpoint: PlcEndpoint) =>
    `${endpoint.ip.includes(":") ? `[${endpoint.ip}]` : endpoint.ip}:${endpoint.port} · Unit ID ${endpoint.unitId}`;

  return <div className="plc-settings-page">
    <PageHeader eyebrow="HỆ THỐNG" title="4 · Kết nối PLC" description="Cài đặt kết nối và theo dõi trạng thái tiếp nhận dữ liệu từ PLC." />
    <Card className="plc-settings-card" loading={loading}>
      <>
        <div className="plc-settings-grid">
          <section className="plc-settings-panel" aria-labelledby="plc-settings-form-title">
            <div className="plc-settings-heading">
              <span className="plc-settings-icon"><ApiOutlined /></span>
              <h2 id="plc-settings-form-title">Cài đặt kết nối</h2>
            </div>
          <Form form={form} layout="vertical" initialValues={DEFAULT_ENDPOINT} disabled={!!busy || loading || registerBusy} onValuesChange={changed} requiredMark={false}>
            <Form.Item name="ip" label="Địa chỉ IP" rules={[{ required: true, message: "Vui lòng nhập địa chỉ IP." }, { validator: (_, value: string) => !value || isPlcIp(value) ? Promise.resolve() : Promise.reject(new Error("Địa chỉ IP chưa hợp lệ. Vui lòng kiểm tra lại.")) }]}>
              <Input placeholder="Ví dụ: 192.168.1.5" autoComplete="off" size="large" disabled={!canEditConnection} />
            </Form.Item>
            <div className="plc-settings-fields">
              <Form.Item name="port" label="Cổng kết nối" rules={[{ required: true, message: "Vui lòng nhập cổng kết nối." }, { type: "integer", min: 1, max: 65535, message: "Cổng kết nối phải từ 1 đến 65535." }]}>
                <InputNumber min={1} max={65535} precision={0} size="large" disabled={!canEditConnection} />
              </Form.Item>
              <Form.Item name="unitId" label="Unit ID" rules={[{ required: true, message: "Vui lòng nhập Unit ID." }, { type: "integer", min: 1, max: 247, message: "Unit ID phải từ 1 đến 247." }]}>
                <InputNumber min={1} max={247} precision={0} size="large" disabled={!canEditConnection} />
              </Form.Item>
            </div>
            {dirty && <p className="plc-settings-dirty" role="status">Thay đổi chưa được lưu.</p>}
            <div className="plc-settings-actions">
              <Button icon={<ApiOutlined />} loading={busy === "test"} disabled={!!busy || !settings || registerBusy} onClick={() => void act("test")}>Kiểm tra kết nối</Button>
              <Button type="primary" icon={<SaveOutlined />} loading={busy === "save"} disabled={!!busy || !canEditConnection || !settings} onClick={() => void act("save")}>Lưu và kết nối</Button>
            </div>
          </Form>
        <div className="plc-settings-feedback" aria-live="polite">
          {error && errorAction !== "test" && <Alert type="error" showIcon title={error} />}
          {notice && <p className="plc-settings-saved"><CheckCircleOutlined /> {notice}</p>}
        </div>
          </section>
          <section className="plc-settings-panel plc-settings-status-panel" aria-labelledby="plc-settings-status-title">
            <h2 id="plc-settings-status-title" className="plc-settings-status-title">Trạng thái PLC</h2>
            <dl className="plc-settings-status" aria-live="polite">
          <div><dt>Địa chỉ vừa kiểm tra</dt><dd>
            <Tag color={busy === "test" ? "processing" : tested === "ok" ? "success" : tested === "failed" ? "error" : "default"}
              icon={busy === "test" ? <ReloadOutlined spin /> : tested === "ok" ? <CheckCircleOutlined /> : undefined}>
              {busy === "test" ? "Đang kiểm tra" : tested === "ok" ? "Kết nối thành công" : tested === "failed" ? "Không thể kết nối" : "Chưa kiểm tra"}
            </Tag>
            {error && errorAction === "test" && <small className="plc-settings-test-error" role="alert">{error}</small>}
            {tested === "ok" && mappingWarning && <Alert type="warning" showIcon style={{ marginTop: 8 }}
              title="Đã đọc được thanh ghi nhưng dữ liệu không khớp register map AS200. Chưa thể thu dữ liệu."
              description={mappingWarning} />}
          </dd></div>
          <div><dt>Cấu hình đã lưu</dt><dd>{settings?.endpoint ? endpointText(settings.endpoint) : "Chưa thiết lập"}</dd></div>
          <div><dt>Endpoint Worker</dt><dd>{worker?.enabled && worker.address ? `${worker.address} · Unit ID ${worker.unitId}` : activeSettings?.activeEndpoint ? endpointText(activeSettings.activeEndpoint) : missing}</dd></div>
          <div><dt>Kết nối TCP / Modbus</dt><dd><Tag color={!monitorError && worker?.connection === "CONNECTED" ? "green" : worker?.connection === "DISCONNECTED" ? "red" : "default"}>{tcpLabel}</Tag></dd></div>
          <div><dt>Thu dữ liệu</dt><dd><Tag color={display.collectionColor}>{display.collectionLabel}</Tag></dd></div>
          <div><dt>Heartbeat PLC</dt><dd>{monitorError ? "Chưa xác minh" : worker ? healthLabels[worker.plcHealth] ?? missing : missing}<small>Giá trị thanh ghi: {missing}</small></dd></div>
          <div><dt>Register Map hiện tại</dt><dd>{worker?.plcHealth === "INVALID_DATA" ? <Tag color="red">Dữ liệu không hợp lệ</Tag> : missing}</dd></div>
          <div><dt>ACK</dt><dd><Tag color={monitorError ? "default" : worker?.ackStatus === "ACKNOWLEDGED" ? "green" : ["FAILED", "RECONCILIATION_REQUIRED"].includes(worker?.ackStatus ?? "") ? "red" : "default"}>{monitorError ? "Chưa xác minh" : worker ? ackLabels[worker.ackStatus] ?? missing : missing}</Tag>
            {["FAILED", "RECONCILIATION_REQUIRED"].includes(worker?.ackStatus ?? "") && <small className="plc-settings-test-error">Giữ nguyên dữ liệu và yêu cầu đang chờ. Không tự gửi lại ACK; liên hệ kỹ thuật để đối chiếu.</small>}
          </dd></div>
            </dl>
            {currentError && <Alert className="plc-status-error" type="error" showIcon title={currentError} />}
            <h3 className="plc-status-subtitle">Giám sát thanh ghi hiện thời</h3>
            <dl className="plc-settings-status plc-live-registers">
              {["TotalCount", "RecordSeq đang công bố", "DataReady", "PC_ReadCount", "PC_ReadDone", "Kết nối gần nhất", "Thời gian đọc gần nhất", "Lỗi gần nhất"].map(label => (
                <div key={label}><dt>{label}</dt><dd>{missing}</dd></div>
              ))}
            </dl>
            <p className="plc-settings-note">Backend chưa cung cấp snapshot thanh ghi và lịch sử thời gian cho giao diện. Trạng thái TCP không xác nhận ACK.</p>
            <h3 className="plc-status-subtitle">Bản ghi tiếp nhận gần nhất</h3>
            <dl className="plc-settings-status">
              <div><dt>Thời gian nhận</dt><dd>{worker?.latest ? formatReportTime(worker.latest.receivedAt) : missing}</dd></div>
              <div><dt>RecordSeq đã tiếp nhận</dt><dd>{worker?.latest?.recordSeq ?? missing}</dd></div>
              <div><dt>TotalCount lúc tiếp nhận</dt><dd>{worker?.latest?.totalCount ?? missing}</dd></div>
            </dl>
          </section>
        </div>
      </>
    </Card>
    <Collapse className="plc-technical-details" items={[{ key: "map", label: "Sơ đồ vùng nhớ và thông tin kỹ thuật", children: <>
      <div className="plc-memory-map">
        <div><b>D25000–D25019</b><span>Vùng điều khiển · FormatVersion, RecordSize, Heartbeat, TotalCount và handshake.</span></div>
        <div><b>D25020–D25259</b><span>Bản ghi 1 · vùng công bố 240 WORD, PC đọc kết quả tại đây.</span></div>
        <div><b>D25260–D25499</b><span>Buffer 2 · do PLC quản lý nội bộ; PC không chọn buffer.</span></div>
      </div>
      <dl className="plc-settings-status plc-technical-status">
        <div><dt>Giao thức</dt><dd>Modbus TCP · Published Record</dd></div>
        <div><dt>Handshake theo Register Map</dt><dd>D25008–D25009: PC_ReadCount · D25011: DataReady · D25012: PC_ReadDone</dd></div>
        <div><dt>Modbus address offset đang dùng</dt><dd>{missing}<small>Địa chỉ D ở trên theo Register Map, offset tham chiếu 0. Chưa có dữ liệu cấu hình runtime để xác nhận.</small></dd></div>
        <div><dt>Thứ tự WORD REAL/DINT</dt><dd>{missing}</dd></div>
        <div><dt>Kiểm tra cấu hình backend</dt><dd>{activeSettings ? activeSettings.technicalReady ? "Cấu hình qua kiểm tra hợp lệ" : "Cần kỹ thuật kiểm tra cấu hình" : missing}<small>Không thay thế xác minh mapping trên PLC hoặc bằng chứng ACK.</small></dd></div>
      </dl>
    </> }]} />
    <Card className="section-card plc-connection-events" title="Nhật ký kết nối">
      <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="Chưa có dữ liệu nhật ký kết nối từ backend" />
    </Card>
    {registerUiEnabled && <PlcRegisterCard form={form} worker={worker} disabled={!!busy || loading} onBusyChange={setRegisterBusy}
      onStopped={async () => { await refreshWorker(); }} />}
  </div>;
}
