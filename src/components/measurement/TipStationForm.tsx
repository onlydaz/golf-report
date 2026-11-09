
import { Alert, Button, InputNumber, Popconfirm, Space, Tag } from "antd";
import type { TipPositionMeasurement } from "../../types/measurement";
import { createInitialTips, recordTipAttempt } from "../../utils/tipWorkflow";

interface TipStationFormProps {
  tips: TipPositionMeasurement[];
  onChange: (tips: TipPositionMeasurement[]) => void;
}

function TipStationForm({ tips, onChange }: TipStationFormProps) {
  const activeIndex = tips.findIndex((tip) => tip.result !== "OK");

  return (
    <>
      <Alert
        type="info"
        showIcon
        message="Khi một vị trí có kết quả NG, các vị trí phía sau sẽ tự động được bỏ qua."
        style={{ marginBottom: 16 }}
      />

      <Popconfirm
        title="Đo lại Trạm Tip từ đầu?"
        description="Toàn bộ dữ liệu Trạm Tip đang nhập sẽ bị xóa."
        okText="Đo lại từ đầu"
        cancelText="Hủy"
        onConfirm={() => onChange(createInitialTips())}
      >
        <Button htmlType="button" style={{ marginBottom: 16 }}>
          Đo lại từ đầu
        </Button>
      </Popconfirm>

      <div className="tip-grid tip-form-grid">
        {tips.map((tip, index) => {
          const active =
            index === activeIndex && tip.result === "INCOMPLETE";

          const valid =
            tip.finalDistance !== null &&
            Number.isFinite(tip.finalDistance) &&
            tip.finalDistance >= 0;

          return (
            <div
              className={`tip-position ${
                tip.result === "SKIPPED" ? "tip-position-skipped" : ""
              }`}
              key={tip.positionNumber}
            >
              <div className="tip-position-header">
                <div className="tip-position-title">
                  Tip {tip.positionNumber}
                </div>

                <Tag
                  color={
                    tip.result === "OK"
                      ? "green"
                      : tip.result === "NG"
                        ? "red"
                        : active
                          ? "blue"
                          : "default"
                  }
                >
                  {tip.result === "INCOMPLETE"
                    ? active
                      ? "Đang kiểm tra"
                      : "Chờ kiểm tra"
                    : tip.result === "SKIPPED"
                      ? "BỎ QUA"
                      : tip.result}
                </Tag>
              </div>

              <label className="measurement-field-label">
                Khoảng cách đo (mm)
              </label>

              <InputNumber
                value={tip.finalDistance}
                disabled={!active}
                min={0}
                step={0.01}
                aria-label={`Khoảng cách Tip ${tip.positionNumber}`}
                onChange={(value) =>
                  onChange(
                    tips.map((item, i) =>
                      i === index
                        ? { ...item, finalDistance: value ?? null }
                        : item
                    )
                  )
                }
                style={{ width: "100%" }}
                placeholder="Nhập khoảng cách"
              />

              <label className="measurement-field-label">
                Số lần kiểm tra lại đã ghi nhận
              </label>

              <InputNumber
                value={tip.retryCount}
                readOnly
                controls={false}
                disabled={tip.result === "SKIPPED"}
                aria-label={`Số lần kiểm tra lại Tip ${tip.positionNumber}`}
                style={{ width: "100%" }}
              />

              <label className="measurement-field-label">
                Kết quả cuối
              </label>

              <Space wrap>
                <Button
                  htmlType="button"
                  color="green"
                  variant="outlined"
                  disabled={!active || !valid}
                  onClick={() =>
                    onChange(recordTipAttempt(tips, index, "OK"))
                  }
                >
                  OK
                </Button>

                <Button
                  htmlType="button"
                  danger
                  disabled={!active || !valid}
                  onClick={() =>
                    onChange(recordTipAttempt(tips, index, "NG"))
                  }
                >
                  NG
                </Button>
              </Space>
            </div>
          );
        })}
      </div>
    </>
  );
}

export default TipStationForm;
