
import { describe, expect, it } from "vitest";
import ExcelJS from "exceljs";
import * as XLSX from "xlsx";

import type { MeasurementDetail } from "../services/measurementService";

import { buildReportTables, formatDecimal, gramsToKilograms, formatLoadKg, isUnmeasuredPoint } from "./as200Report";
import { createCsvBytes, createExcelBytes } from "./as200Export";
import dayjs from "dayjs";
import { formatReportTime, reportDateBoundary } from "./reportTime";

const summaryHeaders = [
  "Mã phép đo", "Mã sản phẩm", "Số gậy", "Thời gian nhận",
  "Mã lượt đo (RecordSeq)", "Tổng số lượt đo", "Kết quả tổng thể",
  "Kết quả TIP", "Tải trọng TIP (kg)", "Kết quả BUTT", "Tải trọng BUTT (kg)",
];

const pointHeaders = [
  "Mã phép đo", "Trạm đo", "Điểm đo", "Vị trí đo (mm)",
  "Kết quả", "Số lượt đo", "Lượt quyết định",
];

const attemptHeaders = [
  "Mã phép đo", "Trạm đo", "Điểm đo",
  "Lượt đo", "PushDist (mm)", "Bend (mm)",
];

const record: MeasurementDetail = {
  id: 18,
  sourceId: 1,
  sessionId: "test",
  recordSeq: 3,
  totalCount: 3,
  productCode: "Gậy, A",
  clubNumber: 0,
  measuredAt: "2026-11-09T02:00:00Z",
  createdAt: "2026-11-09T02:00:00Z",
  overallResult: "NG",
  tipResult: "NG",
  buttResult: "UNMEASURED",
  tipLoad: "123.123456789012345678",
  buttLoad: "0",

  stations: [
    {
      id: 1,
      measurementId: 18,
      stationType: "TIP",
      result: "NG",
      pointCount: 1,
      ngPoint: 1,
      load: "123.123456789012345678",
      createdAt: "",

      points: [
        {
          id: 2,
          stationId: 1,
          pointNumber: 1,
          result: "NG",
          attemptCount: 2,
          decideAttempt: 1,
          position: "-0.000000000000000123",

          attempts: [
            {
              id: 3,
              pointId: 2,
              attemptNumber: 1,
              pushDist: "0",
              bend: "-0.125",
            },
            {
              id: 4,
              pointId: 2,
              attemptNumber: 2,
              pushDist: null,
              bend: "1.234567890123456789",
            },
          ],
        },
      ],
    },
    {
      id: 2,
      measurementId: 18,
      stationType: "BUTT",
      result: "UNMEASURED",
      pointCount: 0,
      ngPoint: 0,
      load: "0",
      createdAt: "",
      points: [
        {
          id: 5,
          stationId: 2,
          pointNumber: 2,
          result: "UNMEASURED",
          attemptCount: 0,
          decideAttempt: null,
          position: null,
          attempts: [],
        },
      ],
    },
  ],
};

describe("AS200 display and export", () => {
  it("converts grams to kilograms exactly without mutating PLC/database values", () => {
    for (const [grams, kilograms] of [["500", "0.5"], ["200", "0.2"], ["0", "0"], ["-1250", "-1.25"],
      ["123.123456789012345678", "0.123123456789012345678"], ["-1.25E-8", "-0.0000000000125"],
      ["1E+6", "1000"], ["00500.000", "0.5"]]) expect(gramsToKilograms(grams)).toBe(kilograms);
    expect(formatLoadKg(500)).toBe("0,5");
    expect(formatLoadKg("200")).toBe("0,2");
    expect(formatLoadKg(null)).toBe("—");
    expect(gramsToKilograms(undefined)).toBeNull();
    expect(gramsToKilograms("NaN")).toBeNull();
    const original = structuredClone(record);
    buildReportTables([record]);
    expect(record).toEqual(original);
  });
  it("preserves independent PLC results and detail slots beyond AttemptCount", async () => {
    const source = structuredClone(record);
    source.overallResult = "OK";
    source.buttResult = "NG";
    source.stations[1].result = "NG";
    source.stations[1].pointCount = 0;
    source.stations[0].points[0].attemptCount = 1;
    source.stations[0].points[0].decideAttempt = 2;
    const tables = buildReportTables([source]);
    expect(tables.measurements[0].OverallResult).toBe("OK");
    expect(tables.measurements[0]["BUTT Result"]).toBe("NG");
    expect(tables.points[1].Result).toBe("UNMEASURED"); // No inferred SKIPPED.
    expect(tables.points[0].AttemptCount).toBe(1);
    expect(tables.points[0].DecideAttempt).toBe(2);
    expect(tables.attempts.map(a => a.AttemptNumber)).toEqual([1, 2]);
    const workbook = new ExcelJS.Workbook();
    await workbook.xlsx.load(await createExcelBytes(tables.measurements, [source]) as unknown as Parameters<typeof workbook.xlsx.load>[0]);
    expect(workbook.worksheets[0].getRow(2).values).toContain("OK");
    expect(source.stations[0].points[0].attemptCount).toBe(1);
  });

  it("excludes unused TIP P2-P4 padding while retaining legitimate BUTT unmeasured points", () => {
    const source = structuredClone(record);
    for (const pointNumber of [2, 3, 4]) {
      source.stations[0].points.push({ ...source.stations[1].points[0], stationId: 1, pointNumber });
    }
    const tables = buildReportTables([source]);
    expect(tables.points.filter(p => p.StationType === "TIP").map(p => p.PointNumber)).toEqual([1]);
    expect(tables.points.filter(p => p.StationType === "BUTT")).toHaveLength(1);
    expect(source.stations[0].points).toHaveLength(4);
  });

  it("distinguishes unused point padding from measured zero and conflicting point data", () => {
    const unused = record.stations[1].points[0];
    expect(isUnmeasuredPoint(unused)).toBe(true);
    expect(isUnmeasuredPoint({ ...unused, position: "0" })).toBe(true);
    expect(isUnmeasuredPoint({ ...unused, position: "12" })).toBe(false);
    expect(isUnmeasuredPoint({ ...unused, attempts: record.stations[0].points[0].attempts })).toBe(false);
    expect(isUnmeasuredPoint({ ...record.stations[0].points[0], position: "0" })).toBe(false);
    const source = structuredClone(record);
    source.stations[1].points[0].position = "0";
    expect(buildReportTables([source]).points[1]["Position (mm)"]).toBe("");
    expect(source.stations[1].points[0].position).toBe("0");
  });
  it("uses an exclusive next-day boundary for single and multiple Vietnam days", () => {
    const start = reportDateBoundary(dayjs("2026-11-10"));
    const end = reportDateBoundary(dayjs("2026-11-10"), true);
    expect(start).toBe("2026-11-09T17:00:00.000Z");
    expect(end).toBe("2026-11-10T17:00:00.000Z");
    expect(reportDateBoundary(dayjs("2026-11-12"), true)).toBe("2026-11-12T17:00:00.000Z");
    // PostgreSQL microseconds are checked by the SELECT-only Rust test; JS Date is millisecond precision.
    expect(Date.parse("2026-11-10T23:59:59.999000+07:00")).toBeLessThan(Date.parse(end));
    expect(Date.parse("2026-11-10T23:59:59.999500+07:00")).toBeLessThan(Date.parse(end));
    expect(Date.parse("2026-11-11T00:00:00.000000+07:00")).toBe(Date.parse(end));
  });
  it("uses Vietnam calendar days and the same timestamp text in UI and export", () => {
    expect(formatReportTime("2026-11-09T17:30:00Z")).toBe("10/11/2026 00:30:00");
    expect(formatReportTime("2026-11-10T00:30:00+07:00")).toBe(formatReportTime("2026-11-09T17:30:00Z"));
    for (const offset of ["-08:00", "+09:00", "+07:00"]) {
      const selected = dayjs(`2026-11-10T12:00:00${offset}`).utcOffset(offset);
      expect(reportDateBoundary(selected)).toBe("2026-11-09T17:00:00.000Z");
      expect(reportDateBoundary(selected, true)).toBe("2026-11-10T17:00:00.000Z");
    }
    expect(buildReportTables([record]).measurements[0].MeasuredAt).toBe(formatReportTime(record.measuredAt));
    expect(record.measuredAt).toBe("2026-11-09T02:00:00Z");
  });

  it("neutralizes dangerous CSV text without changing raw data, XLSX text or decimal strings", async () => {
    for (const product of ["=1+1", "+SUM(1,2)", "-2+3", "@SUM(A1)", "  =1+1", "\tvalue", "\r=1+1", "\n=1+1"]) {
      const source = { ...record, productCode: product };
      const tables = buildReportTables([source]);
      tables.measurements[0]["TIP Load (kg)"] = "-0.123456789012345678";
      const bytes = createCsvBytes(tables.measurements);
      const rows = XLSX.utils.sheet_to_json<Record<string, string>>(XLSX.read(bytes, { type: "array", raw: true }).Sheets.Sheet1);
      expect(rows[0]["Mã sản phẩm"]).toBe(`'${product}`);
      expect(rows[0]["Tải trọng TIP (kg)"]).toBe("-0.123456789012345678");
      expect(source.productCode).toBe(product);
      const workbook = new ExcelJS.Workbook();
      await workbook.xlsx.load(await createExcelBytes(tables.measurements, [source]) as unknown as Parameters<typeof workbook.xlsx.load>[0]);
      // XML parsers normalize CR to LF on the existing ExcelJS round trip.
      // CSV above must preserve the original CR; neither export mutates its source.
      expect(workbook.getWorksheet("Tổng hợp")!.getCell("B2").value).toBe(product.replace(/\r/g, "\n"));
      expect(tables.measurements[0].ProductCode).toBe(product);
    }
    const spoofed = { ...buildReportTables([record]).measurements[0], OverallResult: "@SUM(A1)", "TIP Load (kg)": "=1+1" };
    const rows = XLSX.utils.sheet_to_json<Record<string, string>>(XLSX.read(createCsvBytes([spoofed]), { type: "array", raw: true }).Sheets.Sheet1);
    expect(rows[0]["Kết quả tổng thể"]).toBe("'@SUM(A1)");
    expect(rows[0]["Tải trọng TIP (kg)"]).toBe("'=1+1");
  });
  it("formats decimal strings without losing digits, including zero and scientific notation", () => {
    expect(formatDecimal("1234.123456789012345678")).toBe("1.234,123456789012345678");
    expect(formatDecimal("0")).toBe("0");
    expect(formatDecimal(null)).toBe("—");
    expect(formatDecimal("-1.25E-8")).toBe("-0,0000000125");
    expect(formatDecimal("1E+6")).toBe("1.000.000");
  });

  it("exports actual attempts only and preserves the stored decision, nulls and separate loads", () => {
    const tables = buildReportTables([record]);

    expect(tables.attempts).toHaveLength(2);
    expect(tables.points[0].DecideAttempt).toBe(1);

    expect(tables.points[1]).toMatchObject({
      Result: "UNMEASURED",
      AttemptCount: 0,
      DecideAttempt: "",
      "Position (mm)": "",
    });

    expect(tables.measurements[0]).toMatchObject({
      "TIP Load (kg)": "0.123123456789012345678",
      "BUTT Load (kg)": "0",
      StickNo: 0,
    });

    expect(buildReportTables([])).toEqual({
      measurements: [],
      points: [],
      attempts: [],
    });
  });

  it("round-trips Excel three sheets and exact decimal text", async () => {
    const bytes = await createExcelBytes(
      buildReportTables([record]).measurements,
      [record]
    );

    const workbook = new ExcelJS.Workbook();

    await workbook.xlsx.load(
      bytes as unknown as Parameters<typeof workbook.xlsx.load>[0]
    );

    expect(workbook.worksheets.map(sheet => sheet.name)).toEqual([
      "Tổng hợp", "Điểm đo", "Lượt đo",
    ]);

    [summaryHeaders, pointHeaders, attemptHeaders].forEach((headers, index) => {
      expect(
        headers.map((_, column) =>
          workbook.worksheets[index].getRow(1).getCell(column + 1).value
        )
      ).toEqual(headers);

      expect(workbook.worksheets[index].columnCount).toBe(headers.length);
    });

    expect(workbook.getWorksheet("Tổng hợp")!.rowCount).toBe(2);
    expect(workbook.getWorksheet("Tổng hợp")!.getCell("I2").value).toBe("0.123123456789012345678");
    expect(workbook.getWorksheet("Điểm đo")!.rowCount).toBe(3);
    expect(workbook.getWorksheet("Điểm đo")!.getCell("B2").value).toBe("TIP");
    expect(workbook.getWorksheet("Điểm đo")!.getCell("E2").value).toBe("NG");
    expect(workbook.getWorksheet("Lượt đo")!.rowCount).toBe(3);
    expect(workbook.getWorksheet("Lượt đo")!.getCell("F3").value).toBe("1.234567890123456789");
  });

  it("CSV contains only the flat summary and correctly escapes product codes", () => {
    const bytes = createCsvBytes(buildReportTables([record]).measurements);

    expect(Array.from(bytes.slice(0, 3))).toEqual([0xef, 0xbb, 0xbf]);

    const rows = XLSX.utils.sheet_to_json<Record<string, string>>(
      XLSX.read(bytes, { type: "array", raw: true }).Sheets.Sheet1
    );

    expect(rows).toHaveLength(1);
    expect(Object.keys(rows[0])).toEqual(summaryHeaders);
    expect(rows[0]["Mã sản phẩm"]).toBe(record.productCode);
    expect(rows[0]["Tải trọng TIP (kg)"]).toBe("0.123123456789012345678");
    expect(rows[0]).not.toHaveProperty("PushDist (mm)");
  });

  it("exports CSV nested details without dropping unmeasured points, nulls, zero or decimal precision", () => {
    const source = { ...record, productCode: "=unsafe" };
    const bytes = createCsvBytes(buildReportTables([source]).measurements, [source]);
    const rows = XLSX.utils.sheet_to_json<Record<string, string>>(
      XLSX.read(bytes, { type: "array", raw: true }).Sheets.Sheet1, { defval: "" }
    );
    expect(rows).toHaveLength(3); // Two actual attempts and one point without attempts.
    expect(rows[0]).toMatchObject({
      "Mã sản phẩm": "'=unsafe", "Trạm đo": "TIP", "Tải trọng trạm (kg)": "0.123123456789012345678",
      "Điểm đo": "1", "Lượt quyết định": "1", "PushDist (mm)": "0", "Bend (mm)": "-0.125",
    });
    expect(rows[1]).toMatchObject({ "Lượt đo": "2", "PushDist (mm)": "", "Bend (mm)": "1.234567890123456789" });
    expect(rows[2]).toMatchObject({
      "Trạm đo": "BUTT", "Điểm đo": "2", "Kết quả": "UNMEASURED", "Số lượt đo": "0",
      "Lượt quyết định": "", "Vị trí đo (mm)": "", "Bend (mm)": "",
    });
    expect(source.productCode).toBe("=unsafe");
  });
});
