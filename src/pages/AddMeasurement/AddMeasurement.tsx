
import { ReloadOutlined, SaveOutlined } from "@ant-design/icons";
import { Alert, Button, Card, Form, Space, message } from "antd";
import dayjs, { type Dayjs } from "dayjs";
import { useEffect, useMemo, useState } from "react";

import PageHeader from "../../components/PageHeader";
import ButtStationForm from "../../components/measurement/ButtStationForm";
import GeneralInformationForm from "../../components/measurement/GeneralInformationForm";
import OverallResult from "../../components/measurement/OverallResult";
import TipStationForm from "../../components/measurement/TipStationForm";

import type {
  ButtMeasurement,
  MeasurementInput,
  TipPositionMeasurement,
} from "../../types/measurement";

import {
  calculateOverallResult,
  calculateTipResult,
  validateMeasurementInput,
} from "../../utils/measurementValidation";

import {
  createMeasurement,
  getDatabaseStatus,
} from "../../services/measurementService";

const createInitialTips = (): TipPositionMeasurement[] =>
  [1, 2, 3, 4].map((positionNumber) => ({
    positionNumber,
    finalDistance: null,
    retryCount: null,
    result: "INCOMPLETE",
  }));

const createInitialButt = (): ButtMeasurement => ({
  finalDistance: null,
  result: null,
});

function AddMeasurement() {
  const [form] = Form.useForm();
  const [messageApi, contextHolder] = message.useMessage();

  const [tips, setTips] = useState<TipPositionMeasurement[]>(createInitialTips);
  const [butt, setButt] = useState<ButtMeasurement>(createInitialButt);
  const [saving, setSaving] = useState(false);
  const [databaseReady, setDatabaseReady] = useState(false);
  const [databaseMessage, setDatabaseMessage] = useState(
    "Đang kết nối cơ sở dữ liệu..."
  );

  const tipResult = useMemo(
    () => calculateTipResult(tips),
    [tips]
  );

  const overallResult = useMemo(
    () => calculateOverallResult(tipResult, butt.result),
    [tipResult, butt.result]
  );

  useEffect(() => {
    let active = true;

    const refreshStatus = async () => {
      try {
        const status = await getDatabaseStatus();

        if (active) {
          setDatabaseReady(status.state === "CONNECTED");
          setDatabaseMessage(status.message);
        }
      } catch {
        if (active) {
          setDatabaseReady(false);
          setDatabaseMessage("Không thể đọc trạng thái cơ sở dữ liệu.");
        }
      }
    };

    void refreshStatus();

    const timer = window.setInterval(
      () => void refreshStatus(),
      2000
    );

    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, []);

  const reset = () => {
    form.resetFields();
    form.setFieldValue("measurementTime", dayjs());
    setTips(createInitialTips());
    setButt(createInitialButt());
  };

  const save = async () => {
    let values: {
      productCode: string;
      measurementTime: Dayjs;
      clubNumber: string;
      weight: number;
    };

    try {
      values = await form.validateFields();
    } catch {
      messageApi.error(
        "Vui lòng kiểm tra các trường bắt buộc trong biểu mẫu."
      );
      return;
    }

    const input: MeasurementInput = {
      productCode: values.productCode,
      measurementTime: values.measurementTime.toISOString(),
      clubNumber: values.clubNumber,
      weight: values.weight,
      tips,
      butt,
      tipResult,
      overallResult,
    };

    const errors = validateMeasurementInput(input);

    if (errors.length > 0) {
      messageApi.error(errors[0]);
      return;
    }

    if (!databaseReady) {
      messageApi.error(
        "Cơ sở dữ liệu chưa sẵn sàng. Vui lòng chờ hoặc thử kết nối lại."
      );
      return;
    }

    setSaving(true);

    try {
      const measurementId = await createMeasurement(input);

      messageApi.success(
        `Đã lưu kết quả đo. Mã bản ghi: ${measurementId}.`
      );
    } catch (error: unknown) {
      const errorMessage =
        typeof error === "string"
          ? error
          : "Không thể lưu kết quả đo. Vui lòng kiểm tra PostgreSQL và thử lại.";

      messageApi.error(errorMessage);
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      {contextHolder}

      <PageHeader
        eyebrow="Nhập dữ liệu"
        title="Thêm kết quả đo"
        description="Nhập kết quả đo cuối cùng và số lần kiểm tra lại cho gậy Golf."
        extra={
          <Button
            type="default"
            icon={<ReloadOutlined />}
            onClick={reset}
          >
            Đặt lại
          </Button>
        }
      />

      <Form
        form={form}
        layout="vertical"
        requiredMark={false}
        initialValues={{ measurementTime: dayjs() }}
      >
        <Card className="section-card add-measurement-card">
          <div className="form-section-title">
            A. Thông tin chung
          </div>

          <GeneralInformationForm />
        </Card>

        {!databaseReady && (
          <Alert
            type="warning"
            showIcon
            message={databaseMessage}
            style={{ marginBottom: 20 }}
          />
        )}

        <Card className="section-card add-measurement-card">
          <div className="form-section-title">
            B. Trạm Tip <span className="card-caption">4 vị trí đo</span>
          </div>

          <TipStationForm
            tips={tips}
            onChange={setTips}
          />

          <div className="computed-result">
            <span>Kết quả Tip</span>

            <strong>
              {tipResult === "INCOMPLETE"
                ? "CHƯA HOÀN TẤT"
                : tipResult}
            </strong>
          </div>
        </Card>

        <Card className="section-card add-measurement-card">
          <div className="form-section-title">
            C. Trạm Butt
          </div>

          <ButtStationForm
            butt={butt}
            onChange={setButt}
          />
        </Card>

        <Card className="section-card add-measurement-card">
          <div className="form-section-title">
            D. Kết quả tổng thể
          </div>

          <OverallResult
            tipResult={tipResult}
            buttResult={butt.result}
            overallResult={overallResult}
          />
        </Card>

        <div className="add-measurement-actions">
          <Space>
            <Button
              onClick={reset}
              disabled={saving}
            >
              Đặt lại
            </Button>

            <Button
              type="primary"
              disabled={!databaseReady}
              loading={saving}
              icon={<SaveOutlined />}
              onClick={save}
            >
              Lưu kết quả
            </Button>
          </Space>
        </div>
      </Form>
    </>
  );
}

export default AddMeasurement;
