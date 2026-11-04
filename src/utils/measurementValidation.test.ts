import { describe, expect, it } from "vitest";
import type { MeasurementInput, TipPositionMeasurement } from "../types/measurement";
import { calculateOverallResult, calculateTipResult, validateMeasurementInput, validateTipPosition } from "./measurementValidation";

const tip = (positionNumber: number, result: TipPositionMeasurement["result"] = "OK", retryCount = 0): TipPositionMeasurement => ({
  positionNumber,
  finalDistance: result === "SKIPPED" || result === "INCOMPLETE" ? null : 10,
  retryCount: result === "SKIPPED" || result === "INCOMPLETE" ? null : retryCount,
  result,
});

const completeInput = (tips = [tip(1), tip(2), tip(3), tip(4)]): MeasurementInput => ({
  productCode: "GFL-STEEL-07",
  measurementTime: new Date().toISOString(),
  clubNumber: "01",
  weight: 100,
  tips,
  butt: { finalDistance: 10, result: "OK" },
  tipResult: calculateTipResult(tips),
  overallResult: calculateOverallResult(calculateTipResult(tips), "OK"),
});

describe("measurement result calculation", () => {
  it("returns overall OK when all Tip positions and Butt are OK", () => {
    expect(calculateTipResult([tip(1), tip(2), tip(3), tip(4)])).toBe("OK");
    expect(calculateOverallResult("OK", "OK")).toBe("OK");
  });

  it("returns overall NG when Butt is NG", () => {
    expect(calculateOverallResult("OK", "NG")).toBe("NG");
  });

  it("returns Tip NG when a position is NG and later positions are skipped", () => {
    const tips = [tip(1), tip(2, "NG", 4), tip(3, "SKIPPED"), tip(4, "SKIPPED")];
    expect(calculateTipResult(tips)).toBe("NG");
    expect(calculateOverallResult("NG", "OK")).toBe("NG");
  });

  it("returns INCOMPLETE when a position has not been completed", () => {
    const tips = [tip(1), tip(2), tip(3, "INCOMPLETE"), tip(4, "INCOMPLETE")];
    expect(calculateTipResult(tips)).toBe("INCOMPLETE");
    expect(calculateOverallResult("INCOMPLETE", null)).toBe("INCOMPLETE");
  });
});

describe("measurement validation", () => {
  it("rejects NG before four retries", () => {
    expect(validateTipPosition(tip(2, "NG", 3), 1, [tip(1), tip(2, "NG", 3), tip(3, "SKIPPED"), tip(4, "SKIPPED")]).some((error) => error.includes("phải kiểm tra lại đủ 4 lần"))).toBe(true);
  });

  it("rejects values after an NG position", () => {
    const tips = [tip(1), tip(2, "NG", 4), tip(3), tip(4, "SKIPPED")];
    expect(validateTipPosition(tips[2], 2, tips).some((error) => error.includes("phải là SKIPPED"))).toBe(true);
  });

  it("accepts a complete measurement with four Tip positions", () => {
    expect(validateMeasurementInput(completeInput())).toEqual([]);
  });

  it("requires general information and Butt result", () => {
    const input = completeInput();
    input.productCode = "";
    input.clubNumber = "";
    input.weight = null;
    input.butt = { finalDistance: null, result: null };
    expect(validateMeasurementInput(input)).toEqual(expect.arrayContaining([
      "Mã sản phẩm là bắt buộc.",
      "Số thứ tự gậy là bắt buộc.",
      "Weight là bắt buộc và phải là số không âm.",
      "Khoảng cách cuối của Butt là bắt buộc và phải là số không âm.",
      "Kết quả Butt là bắt buộc.",
    ]));
  });
});
