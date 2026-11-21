import { formatDecimal } from "../../utils/as200Report";
import { measurementValueDescription, presentLoad, presentPosition } from "../../utils/measurementPresentation";
import type { MeasurementScalar, MeasurementStationType, PresentedMeasurementValue } from "../../utils/measurementPresentation";

export function MeasurementValue({ value }: { value: PresentedMeasurementValue }) {
  const badge = value.source === "INVALID" ? "LỖI" : null;
  return <span title={measurementValueDescription(value)} data-value-source={value.source}>
    {formatDecimal(value.value)}{badge && <small style={{ marginInlineStart: 4, fontSize: 10,
      color: "#b91c1c", whiteSpace: "nowrap" }}>{badge}</small>}
  </span>;
}

export function LoadValue({ value, station }: { value: MeasurementScalar; station: MeasurementStationType }) {
  return <MeasurementValue value={presentLoad(value, station)} />;
}

export function PositionValue({ value, station, pointNumber }: {
  value: MeasurementScalar; station: MeasurementStationType; pointNumber: number;
}) {
  return <MeasurementValue value={presentPosition(value, station, pointNumber)} />;
}
