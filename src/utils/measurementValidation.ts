import type { ButtMeasurement, MeasurementInput, MeasurementResult, TipPositionMeasurement } from "../types/measurement";

export const TIP_POSITION_COUNT = 4;
export const MAX_RETRY_COUNT = 4;

export function calculateTipResult(tips: TipPositionMeasurement[]): MeasurementResult {
  if (tips.length !== TIP_POSITION_COUNT || tips.some((tip) => tip.result === "INCOMPLETE" || (tip.result === "NG" && tip.retryCount !== MAX_RETRY_COUNT))) return "INCOMPLETE";
  if (tips.some((tip) => tip.result === "NG")) return "NG";
  if (tips.every((tip) => tip.result === "OK")) return "OK";
  return "INCOMPLETE";
}

export function calculateOverallResult(tipResult: MeasurementResult, buttResult: ButtMeasurement["result"]): MeasurementResult {
  if (tipResult === "NG" || buttResult === "NG") return "NG";
  if (tipResult === "OK" && buttResult === "OK") return "OK";
  return "INCOMPLETE";
}

export function validateTipPosition(tip: TipPositionMeasurement, index: number, tips: TipPositionMeasurement[]): string[] {
  const errors: string[] = [];
  const previousNgIndex = tips.findIndex((item) => item.result === "NG");
  if (previousNgIndex >= 0 && index > previousNgIndex && tip.result !== "SKIPPED") errors.push(`Vị trí Tip ${index + 1} phải là SKIPPED sau vị trí NG.`);
  if (tip.result === "SKIPPED") {
    if (previousNgIndex < 0 || index <= previousNgIndex) errors.push(`Tip ${index + 1} chỉ được SKIPPED sau vị trí NG.`);
    if (tip.finalDistance !== null || tip.retryCount !== null) errors.push(`Vị trí Tip ${index + 1} bị bỏ qua không được có dữ liệu.`);
    return errors;
  }
  if (tip.result === "INCOMPLETE") errors.push(`Tip ${index + 1} chưa hoàn tất kiểm tra.`);
  if (tip.positionNumber !== index + 1) errors.push("Các vị trí Tip phải liên tiếp từ 1 đến 4.");
  if (tip.finalDistance === null || !Number.isFinite(tip.finalDistance) || tip.finalDistance < 0) errors.push(`Khoảng cách cuối của Tip ${index + 1} phải là số không âm.`);
  if (tip.retryCount === null || !Number.isInteger(tip.retryCount) || tip.retryCount < 0 || tip.retryCount > MAX_RETRY_COUNT) errors.push(`Số lần kiểm tra lại của Tip ${index + 1} phải từ 0 đến 4.`);
  if (tip.result === "NG" && tip.retryCount !== MAX_RETRY_COUNT) errors.push(`Tip ${index + 1} có kết quả NG phải kiểm tra lại đủ 4 lần.`);
  return errors;
}

export function validateMeasurementInput(input: MeasurementInput): string[] {
  const errors: string[] = [];
  if (!input.productCode.trim()) errors.push("Mã sản phẩm là bắt buộc.");
  if (!input.clubNumber.trim()) errors.push("Số thứ tự gậy là bắt buộc.");
  if (!input.measurementTime || Number.isNaN(Date.parse(input.measurementTime))) errors.push("Thời gian đo không hợp lệ.");
  if (input.measurementTime && new Date(input.measurementTime).getTime() > Date.now()) errors.push("Thời gian đo không được ở tương lai.");
  if (input.weight === null || !Number.isFinite(input.weight) || input.weight < 0) errors.push("Weight là bắt buộc và phải là số không âm.");
  if (input.tips.length !== TIP_POSITION_COUNT) errors.push("Phải có đúng 4 vị trí Tip.");
  input.tips.forEach((tip, index) => errors.push(...validateTipPosition(tip, index, input.tips)));
  if (input.butt.finalDistance === null || !Number.isFinite(input.butt.finalDistance) || input.butt.finalDistance < 0) errors.push("Khoảng cách cuối của Butt là bắt buộc và phải là số không âm.");
  if (input.butt.result !== "OK" && input.butt.result !== "NG") errors.push("Kết quả Butt là bắt buộc.");
  if (input.tipResult === "INCOMPLETE" || input.overallResult === "INCOMPLETE") errors.push("Chưa nhập đủ dữ liệu để xác định kết quả.");
  return [...new Set(errors)];
}

export function isMeasurementComplete(input: MeasurementInput): boolean {
  return validateMeasurementInput(input).length === 0;
}
