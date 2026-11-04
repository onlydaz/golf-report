
import { InputNumber, Select } from "antd";
import type { ButtMeasurement } from "../../types/measurement";

interface ButtStationFormProps {
  butt: ButtMeasurement;
  onChange: (butt: ButtMeasurement) => void;
}

function ButtStationForm({ butt, onChange }: ButtStationFormProps) {
  return (
    <div className="butt-form-grid">
      <div>
        <label className="measurement-field-label">
          Khoảng cách cuối (mm)
        </label>

        <InputNumber
          value={butt.finalDistance ?? undefined}
          min={0}
          step={0.01}
          onChange={(value) =>
            onChange({
              ...butt,
              finalDistance: value ?? null,
            })
          }
          style={{ width: "100%" }}
          placeholder="Nhập khoảng cách"
        />
      </div>

      <div>
        <label className="measurement-field-label">
          Kết quả
        </label>

        <Select
          value={butt.result ?? undefined}
          onChange={(value) =>
            onChange({
              ...butt,
              result: value,
            })
          }
          style={{ width: "100%" }}
          placeholder="Chọn kết quả"
          options={[
            { value: "OK", label: "OK" },
            { value: "NG", label: "NG" },
          ]}
        />
      </div>
    </div>
  );
}

export default ButtStationForm;
