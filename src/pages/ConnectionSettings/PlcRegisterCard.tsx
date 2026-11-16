import { Alert, Button, Card, Form, Input, InputNumber, Modal, Select, Space } from "antd";
import type { FormInstance } from "antd";
import { useEffect, useRef, useState } from "react";
import type { As200Status } from "../../services/as200Service";
import {
  getPlcRegisterAccess, isPlcIp, readPlcRegister, stopPlcCollection, writePlcRegister,
  type PlcEndpoint, type PlcRegisterAccess, type PlcRegisterSelection, type PlcRegisterWriteReport,
} from "../../services/plcConnectionService";

interface Props {
  form: FormInstance<PlcEndpoint>;
  worker?: As200Status;
  disabled: boolean;
  onBusyChange: (busy: boolean) => void;
  onStopped: () => Promise<void>;
}

export default function PlcRegisterCard({ form, worker, disabled, onBusyChange, onStopped }: Props) {
  const ip = Form.useWatch("ip", form) as string | undefined;
  const port = Form.useWatch("port", form) as number | undefined;
  const unitId = Form.useWatch("unitId", form) as number | undefined;
  const [kind, setKind] = useState<"BOOL" | "WORD">("WORD");
  const [address, setAddress] = useState("D25000");
  const [value, setValue] = useState<number | null>(null);
  const [access, setAccess] = useState<PlcRegisterAccess>();
  const [before, setBefore] = useState<number>();
  const [report, setReport] = useState<PlcRegisterWriteReport>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const writeSent = useRef(false);
  const beforeKey = useRef("");
  const selectionKey = JSON.stringify([ip, port, unitId, kind, address]);
  const mounted = useRef(true);
  const [confirmation, setConfirmation] = useState<{
    selection: PlcRegisterSelection; access: PlcRegisterAccess; value: number; before: number;
  }>();
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);

  // This command only inspects local policy/Worker state; it never contacts PLC.
  useEffect(() => {
    let active = true;
    setAccess(undefined); setBefore(undefined); setReport(undefined); setError("");
    if (!ip || !isPlcIp(ip) || !Number.isInteger(port) || !Number.isInteger(unitId) || !address) return;
    const timer = setTimeout(() => {
      void getPlcRegisterAccess({ endpoint: { ip, port: port!, unitId: unitId! }, kind, address })
        .then((result) => { if (active) setAccess(result); })
        .catch((e: unknown) => { if (active) setError(typeof e === "string" ? e : "Không kiểm tra được quyền thao tác thanh ghi."); });
    }, 250);
    return () => { active = false; clearTimeout(timer); };
  }, [ip, port, unitId, kind, address, worker?.enabled, worker?.state]);

  const start = () => {
    if (inFlight.current) return false;
    inFlight.current = true; setBusy(true); onBusyChange(true); setError(""); return true;
  };
  const finish = () => {
    inFlight.current = false;
    writeSent.current = false;
    if (mounted.current) { setBusy(false); onBusyChange(false); }
  };
  const fail = (e: unknown) => {
    if (mounted.current && !(e && typeof e === "object" && "errorFields" in e)) {
      setError(typeof e === "string" ? e : "Thao tác không hoàn tất. Không tự gửi lại lệnh ghi; đọc lại và đối chiếu ISPSoft.");
    }
  };
  const read = async () => {
    if (!start()) return;
    try {
      const endpoint = await form.validateFields();
      const result = await readPlcRegister({ endpoint, kind, address });
      if (mounted.current) { beforeKey.current = selectionKey; setBefore(result.value); setAccess(result.access); }
    } catch (e) { fail(e); } finally { finish(); }
  };
  const prepareWrite = async () => {
    if (before === undefined || beforeKey.current !== selectionKey || value === null || !access || !start()) return;
    try {
      const endpoint = await form.validateFields();
      // Refresh backend authorization before showing a confirmation. Backend
      // repeats every check under its gate immediately before the actual write.
      const selection = { endpoint, kind, address };
      const current = await getPlcRegisterAccess(selection);
      if (mounted.current) setAccess(current);
      if (!current.writeAllowed) throw current.writeBlockReason;
      if (mounted.current) {
        setConfirmation({ selection, access: current, value, before });
      }
    } catch (e) { if (mounted.current) setAccess(undefined); fail(e); finish(); }
  };
  const write = async (confirmed: boolean) => {
    if (!confirmed || writeSent.current || !confirmation || !confirmation.access.writeAllowed) return;
    writeSent.current = true;
    setReport(undefined);
    try {
      const result = await writePlcRegister({
        selection: confirmation.selection, value: confirmation.value, expectedBefore: confirmation.before,
        // The explicit modal action supplies the existing operator confirmations
        // together. Backend still independently enforces all write permissions.
        programId: confirmation.access.programId, mappingConfirmed: confirmed,
        deviceConfirmed: confirmed, machineSafeConfirmed: confirmed,
        pduAddress: confirmation.access.pduAddress, addressOffset: confirmation.access.addressOffset,
      });
      if (mounted.current) setReport(result);
    } catch (e) { fail(e); } finally {
      // Each write requires a fresh manual read and a new confirmation, including
      // after timeout/unknown outcome. No retries, polling or automatic reset.
      if (mounted.current) { setBefore(undefined); setConfirmation(undefined); }
      finish();
    }
  };
  const stop = async () => {
    if (!start()) return;
    try { await stopPlcCollection(); await onStopped(); } catch (e) { fail(e); } finally { finish(); }
  };
  const canWrite = access?.writeAllowed && worker && !worker.enabled && worker.state !== "STOPPING"
    && before !== undefined && beforeKey.current === selectionKey
    && value !== null && Number.isInteger(value) && access.allowedValues.includes(value);
  const writeSucceeded = report?.writeStatus === "ECHO_CONFIRMED" && report.readbackMatches === true;
  const resultText = !report ? "" : report.writeStatus === "UNCONFIRMED"
    ? "Chưa xác nhận PLC đã nhận lệnh. Kiểm tra ISPSoft trước khi ghi lại."
    : report.writeStatus === "REFUSED" ? report.message || "PLC từ chối lệnh ghi."
    : writeSucceeded ? `Đã ghi ${report.access.address} = ${report.requested}, đọc lại khớp.`
    : "Không thể xác nhận kết quả ghi. Kiểm tra ISPSoft trước khi ghi lại.";
  const blockReason = worker?.state === "STOPPING" ? "Worker đang dừng. Chưa được phép ghi."
    : worker?.enabled ? "Worker đang chạy. Dừng thu trước khi ghi."
    : access?.writeBlockReason;

  return <Card title="Đọc/Ghi thanh ghi PLC" className="plc-settings-card" style={{ marginTop: 24 }}>
    <Space orientation="vertical" size="middle" style={{ width: "100%" }}>
      <Space wrap align="end">
        <div><label htmlFor="plc-register-kind">Loại thanh ghi</label><br />
          <Select id="plc-register-kind" value={kind} disabled={busy || disabled} style={{ width: 150 }}
            options={[{ value: "BOOL", label: "BOOL (M)" }, { value: "WORD", label: "WORD (D)" }]}
            onChange={(next) => { setKind(next); setAddress(next === "BOOL" ? "M0" : "D25000"); setValue(null); }} /></div>
        <div><label htmlFor="plc-register-address">Địa chỉ</label><br />
          <Input id="plc-register-address" value={address} disabled={busy || disabled} placeholder="M0, M100, D25000, D200"
            onChange={(e) => setAddress(e.target.value.toUpperCase())} /></div>
        <div><label htmlFor="plc-register-value">Giá trị {kind === "WORD" ? "WORD (0–65535)" : "BOOL"}</label><br />
          {kind === "BOOL" ? <Select id="plc-register-value" value={value} disabled={busy || disabled} style={{ width: 150 }}
            placeholder="Chọn ON/OFF" options={[{ value: 0, label: "OFF (0)" }, { value: 1, label: "ON (1)" }]} onChange={setValue} />
            : <InputNumber id="plc-register-value" value={value} disabled={busy || disabled} min={0} max={65535} precision={0} onChange={setValue} />}</div>
      </Space>
      <Space wrap>
        <Button disabled={busy || disabled} onClick={() => void read()}>Đọc</Button>
        <Button type="primary" danger disabled={busy || disabled || !canWrite} onClick={() => void prepareWrite()}>Ghi</Button>
        {busy && <span role="status">Đang thực hiện…</span>}
      </Space>
      {before !== undefined && <p role="status" style={{ margin: 0 }}>Đã đọc {address} = {before}.</p>}
      {blockReason && blockReason !== error && <Alert type="warning" showIcon title={blockReason}
        action={worker?.enabled ? <Button size="small" disabled={busy || disabled} onClick={() => void stop()}>Dừng thu</Button> : undefined} />}
      {error && <Alert type="error" showIcon title={error} />}
      {report && <Alert showIcon type={writeSucceeded ? "success" : "warning"} title={resultText} />}
    </Space>
    <Modal title="Xác nhận ghi" open={!!confirmation} okText="Xác nhận ghi" cancelText="Hủy"
      okButtonProps={{ danger: true, disabled: !confirmation?.access.writeAllowed || !worker || worker.enabled || worker.state === "STOPPING" }}
      onOk={() => { setConfirmation(undefined); void write(true); }}
      onCancel={() => { setConfirmation(undefined); finish(); }} maskClosable={false}>
      {confirmation && <p>
        {confirmation.selection.endpoint.ip.includes(":") ? `[${confirmation.selection.endpoint.ip}]` : confirmation.selection.endpoint.ip}:{confirmation.selection.endpoint.port}
        {" · "}{confirmation.selection.address} = {confirmation.selection.kind === "BOOL"
          ? confirmation.value === 1 ? "ON (1)" : "OFF (0)"
          : confirmation.value}
      </p>}
    </Modal>
  </Card>;
}
