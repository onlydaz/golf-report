import { Alert, Button, Card, Descriptions, Input, Space, Tag, message } from "antd";
import { useEffect, useState } from "react";
import PageHeader from "../../components/PageHeader";
import { getPlcStatus, startPlcCapture, type PlcStatus } from "../../services/plcService";

const formatNumber = (value: string | null | undefined) => value == null ? "—" : Number(value).toLocaleString("vi-VN", { maximumFractionDigits: 2 });
const resultTag = (result?: string) => <Tag color={result === "OK" ? "green" : result === "NG" ? "red" : "default"}>{result === "SKIPPED" ? "BỎ QUA" : result ?? "Chờ kết quả"}</Tag>;

function AddMeasurement() {
  const [status, setStatus] = useState<PlcStatus | null>(null);
  const [product, setProduct] = useState("");
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [messageApi, contextHolder] = message.useMessage();
  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      try {
        const next = await getPlcStatus();
        if (active) { setStatus(next); setError(null); }
      } catch (err) {
        if (active) setError(typeof err === "string" ? err : "Không đọc được trạng thái PLC.");
      } finally {
        if (active) timer = setTimeout(() => void refresh(), 500);
      }
    };
    void refresh();
    return () => { active = false; clearTimeout(timer); };
  }, []);
  const start = async () => {
    setStarting(true);
    try {
      await startPlcCapture(product);
      setStatus(await getPlcStatus());
      messageApi.success("Đã xác nhận mã sản phẩm; chờ phép đo mới.");
    } catch (err) { messageApi.error(typeof err === "string" ? err : "Không thể bắt đầu ghi nhận."); }
    finally { setStarting(false); }
  };
  const data = status?.data;
  const tipResult = data ? data.tipPositions.some(tip => tip.result === "NG") ? "NG" : "OK" : undefined;
  const overall = data ? tipResult === "NG" || data.buttResult === "NG" ? "NG" : "OK" : undefined;
  const connection = status?.connection === "CONNECTED" ? "Đã kết nối" : status?.connection === "DISCONNECTED" ? "Mất kết nối" : "Đang kết nối";
  return <>
    {contextHolder}
    <PageHeader eyebrow="PLC" title="Ghi nhận kết quả đo" description="Tự động tiếp nhận, hiển thị và lưu trữ kết quả đo từ PLC theo mã sản phẩm." />
    {error && <Alert type="error" showIcon message={error} style={{ marginBottom: 16 }} />}
    <Card className="section-card add-measurement-card">
      <div className="form-section-title">A. Thông tin kết nối</div>
      <Descriptions column={2} items={[
        { key: "connection", label: "Trạng thái PLC", children: <Tag color={status?.connection === "CONNECTED" ? "green" : "orange"}>{connection}</Tag> },
        { key: "address", label: "Địa chỉ PLC", children: status ? `${status.address} · Unit ID ${status.unit}` : "—" },
        { key: "received", label: "Kết quả", children: data ? `Đã nhận · Sequence ${status?.sequenceId}` : "Đang chờ kết quả" },
      ]} />
    </Card>
    <Card className="section-card add-measurement-card">
      <div className="form-section-title">B. Thông tin sản phẩm</div>
      <label className="measurement-field-label" htmlFor="plc-product">Mã sản phẩm</label>
      <Space wrap>
        <Input id="plc-product" value={product} maxLength={50} placeholder="Nhập mã sản phẩm" disabled={starting || status?.busy} onChange={event => setProduct(event.target.value)} onPressEnter={() => { if (product.trim() && !starting && !status?.busy) void start(); }} />
        <Button type="primary" loading={starting} disabled={!product.trim() || !status || status.busy || status.connection !== "CONNECTED"} onClick={() => void start()}>Bắt đầu ghi nhận</Button>
      </Space>
      <p>Mã đã xác nhận: <strong>{status?.productCode ?? "Chưa xác nhận"}</strong></p>
      <p style={{ color: "#829ab1" }}>Hệ thống sẽ tự động lưu các kết quả đo mới theo mã đã xác nhận.</p>
    </Card>
    <Card className="section-card add-measurement-card">
      <div className="form-section-title">C. Dữ liệu nhận tự động</div>
      <Descriptions column={2} items={[
        { key: "product", label: "Mã sản phẩm phép đo", children: data?.productCode ?? "—" },
        { key: "club", label: "Số thứ tự gậy", children: data?.clubNumber ?? "—" },
        { key: "time", label: "Thời gian nhận", children: data ? new Date(data.measuredAt).toLocaleString("vi-VN") : "—" },
        { key: "weight", label: "Khối lượng đè gậy (Weight)", children: formatNumber(data?.weight) },
      ]} />
      <div style={{ marginTop: 20, marginBottom: 16 }}>
  <strong>Trạm Tip</strong>
</div>

<div className="tip-grid tip-form-grid">
        {[1, 2, 3, 4].map(position => {
          const tip = data?.tipPositions.find(item => item.positionNumber === position);
          return <div className={`tip-position ${tip?.result === "SKIPPED" ? "tip-position-skipped" : ""}`} key={position}>
            <div className="tip-position-header"><div className="tip-position-title">Tip {position}</div></div>
            <label className="measurement-field-label">Khoảng cách đo (mm)</label><div>{formatNumber(tip?.finalDistance)}</div>
            <label className="measurement-field-label">Số lần kiểm tra lại đã ghi nhận</label><div>{tip?.retryCount ?? "—"}</div>
            <label className="measurement-field-label">Kết quả cuối</label>{resultTag(tip?.result)}
          </div>;
        })}
      </div>
      <div className="computed-result"><span>Kết quả Tip</span>{resultTag(tipResult)}</div>
      <div style={{ marginTop: 24 }}></div>
      <Descriptions title="Trạm Butt" items={[
        { key: "distance", label: "Khoảng cách (mm)", children: formatNumber(data?.buttDistance) },
        { key: "result", label: "Kết quả", children: resultTag(data?.buttResult) },
      ]} />
      <div className="computed-result"><span>Kết quả tổng thể</span>{resultTag(overall)}</div>
    </Card>
    <Card className="section-card add-measurement-card">
      <div className="form-section-title">D. Trạng thái lưu dữ liệu</div>
      <Alert showIcon type={status?.phase === "ERROR" ? "error" : status?.phase === "SAVED" ? "success" : "info"} message={status?.message ?? "Đang kết nối..."} />
      {status?.measurementId != null && <p>Mã bản ghi: <strong>{status.measurementId}</strong></p>}
    </Card>
  </>;
}
export default AddMeasurement;
