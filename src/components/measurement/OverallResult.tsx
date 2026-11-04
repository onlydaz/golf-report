import { Alert, Tag } from "antd";
import type { ButtMeasurement, MeasurementResult } from "../../types/measurement";

function OverallResult({
  tipResult,
  buttResult,
  overallResult,
}: {
  tipResult: MeasurementResult;
  buttResult: ButtMeasurement["result"];
  overallResult: MeasurementResult;
}) {
  const color =
    overallResult === "OK"
      ? "green"
      : overallResult === "NG"
        ? "red"
        : "gold";

  const formatResult = (result: MeasurementResult | ButtMeasurement["result"]) =>
    result === "INCOMPLETE" || result === null ? "CHƯA HOÀN TẤT" : result;

  return (
    <div className="overall-result">
      <div>
        <div className="result-box-label">Kết quả Tip</div>
        <Tag
          color={tipResult === "OK" ? "green" : tipResult === "NG" ? "red" : "gold"}
          className="overall-tag"
        >
          {formatResult(tipResult)}
        </Tag>
      </div>

      <div>
        <div className="result-box-label">Kết quả Butt</div>
        <Tag
          color={buttResult === "OK" ? "green" : buttResult === "NG" ? "red" : "gold"}
          className="overall-tag"
        >
          {formatResult(buttResult)}
        </Tag>
      </div>

      <div>
        <div className="result-box-label">Kết quả tổng thể</div>
        <Tag color={color} className="overall-tag">
          {formatResult(overallResult)}
        </Tag>
      </div>

      {overallResult === "INCOMPLETE" && (
        <Alert
          type="warning"
          showIcon
          message="Vui lòng hoàn tất dữ liệu Tip và Butt để xác định kết quả tổng thể."
        />
      )}
    </div>
  );
}

export default OverallResult;