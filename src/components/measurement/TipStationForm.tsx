
import { Alert, InputNumber, Select, Tag } from "antd";
import type {
  TipPositionMeasurement,
  TipPositionResult,
} from "../../types/measurement";
import { MAX_RETRY_COUNT } from "../../utils/measurementValidation";

interface TipStationFormProps {
  tips: TipPositionMeasurement[];
  onChange: (tips: TipPositionMeasurement[]) => void;
}

function TipStationForm({ tips, onChange }: TipStationFormProps) {
  const updateTip = (
    index: number,
    patch: Partial<TipPositionMeasurement>
  ) => {
    const next = tips.map((tip, tipIndex) =>
      tipIndex === index ? { ...tip, ...patch } : tip
    );

    if (patch.result === "NG") {
      for (
        let tipIndex = index + 1;
        tipIndex < next.length;
        tipIndex += 1
      ) {
        next[tipIndex] = {
          positionNumber: tipIndex + 1,
          finalDistance: null,
          retryCount: null,
          result: "SKIPPED",
        };
      }
    }

    if (patch.result === "OK" && tips[index]?.result === "NG") {
      for (
        let tipIndex = index + 1;
        tipIndex < next.length;
        tipIndex += 1
      ) {
        next[tipIndex] = {
          ...next[tipIndex],
          result: "INCOMPLETE" as TipPositionResult,
        };
      }
    }

    onChange(next);
  };

  const firstNgIndex = tips.findIndex(
    (tip) => tip.result === "NG"
  );

  return (
    <>
      <Alert
        type="info"
        showIcon
        message="Khi một vị trí có kết quả NG, các vị trí phía sau sẽ tự động được bỏ qua."
        style={{ marginBottom: 16 }}
      />

      <div className="tip-grid tip-form-grid">
        {tips.map((tip, index) => {
          const skipped = tip.result === "SKIPPED";

          const blocked =
            firstNgIndex >= 0 && index > firstNgIndex;

          return (
            <div
              className={`tip-position ${
                skipped ? "tip-position-skipped" : ""
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
                        : "default"
                  }
                >
                  {tip.result}
                </Tag>
              </div>

              <label className="measurement-field-label">
                Khoảng cách cuối (mm)
              </label>

              <InputNumber
                value={tip.finalDistance ?? undefined}
                disabled={skipped || blocked}
                min={0}
                step={0.01}
                onChange={(value) =>
                  updateTip(index, {
                    finalDistance: value ?? null,
                  })
                }
                style={{ width: "100%" }}
                placeholder="Nhập khoảng cách"
              />

              <label className="measurement-field-label">
                Số lần kiểm tra lại
              </label>

              <InputNumber
                value={tip.retryCount ?? undefined}
                disabled={skipped || blocked}
                min={0}
                max={MAX_RETRY_COUNT}
                step={1}
                precision={0}
                onChange={(value) =>
                  updateTip(index, {
                    retryCount: value ?? null,
                  })
                }
                style={{ width: "100%" }}
                placeholder="0 - 4"
              />

              <label className="measurement-field-label">
                Kết quả cuối
              </label>

              <Select
                value={tip.result}
                disabled={skipped || blocked}
                onChange={(value: TipPositionResult) =>
                  updateTip(index, {
                    result: value,
                  })
                }
                style={{ width: "100%" }}
                options={[
                  {
                    value: "INCOMPLETE",
                    label: "Chưa hoàn tất",
                  },
                  {
                    value: "OK",
                    label: "OK",
                  },
                  {
                    value: "NG",
                    label: "NG",
                  },
                ]}
              />
            </div>
          );
        })}
      </div>
    </>
  );
}

export default TipStationForm;
