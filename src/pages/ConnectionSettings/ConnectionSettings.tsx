import { ApiOutlined, CheckCircleOutlined, ReloadOutlined, SaveOutlined } from "@ant-design/icons";
import { Alert, Button, Card, Form, Input, InputNumber, Modal, Tag } from "antd";
import { useEffect, useRef, useState } from "react";
import PageHeader from "../../components/PageHeader";
import { getAs200WorkerStatus, type As200Status } from "../../services/as200Service";
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
  const [worker, setWorker] = useState<As200Status>();
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
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const value = await getAs200WorkerStatus();
        if (!disposed) setWorker(value);
      } catch {
        if (!disposed) setWorker(undefined);
      } finally {
        if (!disposed) timer = setTimeout(() => void poll(), 2000);
      }
    };
    void poll();
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
          const current = await getAs200WorkerStatus();
          setSettings(await getPlcConnectionSettings());
          setDirty(false);
          setWorker(current);
          setNotice("Đã lưu cấu hình và bắt đầu thu dữ liệu.");
        }
      } else {
        setNotice("Đang kết nối với máy đo...");
        await connect(values, settings.revision);
        const saved = await getPlcConnectionSettings();
        const current = await getAs200WorkerStatus();
        if (mounted.current) { setSettings(saved); if (saved.endpoint) form.setFieldsValue(saved.endpoint); setDirty(false); setWorker(current); setNotice(""); }
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
  const receiving = !worker ? "Chưa xác định"
    : worker.state === "STOPPING" ? "Đang dừng thu dữ liệu"
    : worker.state === "STOPPED" ? worker.message.includes("dữ liệu chưa lưu") ? "Đã dừng; còn dữ liệu chờ phục hồi" : "Đã dừng thu dữ liệu"
    : !worker.enabled ? "Chưa bật"
    : worker.pauseReason ? "Tạm dừng"
    : worker.connection === "CONNECTED" && worker.plcHealth === "RUNNING" ? "Đang thu dữ liệu"
    : worker.plcHealth === "INVALID_DATA" ? "Đã kết nối, dữ liệu không khớp mapping"
    : worker.connection === "DISCONNECTED" ? "Mất kết nối"
    : "Đang chờ thiết bị";
  const endpointText = (endpoint: PlcEndpoint) =>
    `${endpoint.ip.includes(":") ? `[${endpoint.ip}]` : endpoint.ip}:${endpoint.port} · Unit ID ${endpoint.unitId}`;

  return <div className="plc-settings-page">
    <PageHeader eyebrow="HỆ THỐNG" title="Cài đặt kết nối" description="Thiết lập thông tin để kết nối với thiết bị đo (PLC)." />
    <Card className="plc-settings-card" loading={loading}>
      {!settings && !loading ? <Alert type="error" showIcon title={error || "Không thể đọc cấu hình. Vui lòng liên hệ kỹ thuật."} /> : <>
        <div className="plc-settings-grid">
          <section className="plc-settings-panel" aria-labelledby="plc-settings-form-title">
            <div className="plc-settings-heading">
              <span className="plc-settings-icon"><ApiOutlined /></span>
              <h2 id="plc-settings-form-title">Thông tin PLC</h2>
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
            <h2 id="plc-settings-status-title" className="plc-settings-status-title">Trạng thái kết nối</h2>
            <dl className="plc-settings-status" aria-live="polite">
          <div><dt>Kết quả kiểm tra</dt><dd>
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
          <div><dt>Nhận dữ liệu đo</dt><dd>{receiving}
            {!worker && <small>Chưa lấy được trạng thái bộ thu dữ liệu.</small>}
            {worker && !worker.enabled && <small>Kiểm tra lại hồ sơ AS200 và trạng thái PLC để tiếp tục.</small>}
            {worker?.plcHealth === "INVALID_DATA" && worker.message && <small role="alert">{worker.message}</small>}
          </dd></div>
            </dl>
          </section>
        </div>
      </>}
    </Card>
    {registerUiEnabled && <PlcRegisterCard form={form} worker={worker} disabled={!!busy || loading} onBusyChange={setRegisterBusy}
      onStopped={async () => { setWorker(await getAs200WorkerStatus()); }} />}
  </div>;
}
