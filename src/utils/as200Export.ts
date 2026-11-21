
import ExcelJS from "exceljs";
import * as XLSX from "xlsx";

import type { MeasurementDetail } from "../services/measurementService";
import { buildReportTables, displayPointResult, isBusinessPoint, isUnmeasuredPoint } from "./as200Report";
import { exportMeasurementValue, measurementValueDescription, presentLoad, presentPosition } from "./measurementPresentation";
import type { PresentedMeasurementValue } from "./measurementPresentation";

type ExportValue = string | number;
type ExportRow = Record<string, ExportValue>;

const exportHeaders = [
  "Measurement ID",
  "ProductCode",
  "StickNo",
  "MeasuredAt",
  "RecordSeq",
  "TotalCount",
  "OverallResult",
  "TIP Result",
  "TIP Load (kg)",
  "BUTT Result",
  "BUTT Load (kg)",
];

const headerLabels: Record<string, string> = {
  "Measurement ID": "Mã phép đo",
  ProductCode: "Mã sản phẩm",
  StickNo: "Số gậy",
  MeasuredAt: "Thời gian nhận",
  RecordSeq: "Mã lượt đo (RecordSeq)",
  TotalCount: "Tổng số lượt đo",
  OverallResult: "Kết quả tổng thể",
  "TIP Result": "Kết quả TIP",
  "TIP Load (kg)": "Lực đè TIP (kg)",
  "BUTT Result": "Kết quả BUTT",
  "BUTT Load (kg)": "Lực đè BUTT (kg)",
  StationType: "Trạm đo",
  "Station Load (kg)": "Lực đè trạm (kg)",
  "Station Result": "Kết quả trạm",
  PointNumber: "Điểm đo",
  "Position (mm)": "Vị trí đo (mm)",
  Result: "Kết quả",
  AttemptCount: "Số lượt đo",
  DecideAttempt: "Lượt quyết định",
  AttemptNumber: "Lượt đo",
};

const headerLabel = (key: string) => headerLabels[key] ?? key;

// Tính độ rộng cột theo tiêu đề và dữ liệu
const calculateColumnWidth = (
  rows: ExportRow[],
  header: string
): number => {
  let maxLength = headerLabel(header).length;

  for (const row of rows) {
    const value = String(row[header] ?? "");
    maxLength = Math.max(maxLength, value.length);
  }

  return Math.min(Math.max(maxLength + 4, 12), 60);
};

// Xuất CSV UTF-8 BOM
export const createCsvBytes = (rows: ExportRow[], details?: MeasurementDetail[]): Uint8Array => {
  const detailHeaders = ["StationType", "Station Load (kg)", "Station Result", "PointNumber",
    "Position (mm)", "Result", "AttemptCount", "DecideAttempt", "AttemptNumber", "PushDist (mm)", "Bend (mm)"];
  // CSV is flat: repeat the measurement anchor for each actual point/attempt.
  // Keep an empty detail row when a station/point has no samples; NULL stays blank.
  if (details) {
    const byId = new Map(buildReportTables(details).measurements.map(row => [row["Measurement ID"], row]));
    rows = details.flatMap(item => {
      const base = byId.get(item.id)!;
      if (!item.stations.length) return [base];
      return item.stations.flatMap(station => {
        const stationRow = { ...base, StationType: station.stationType,
          "Station Load (kg)": exportMeasurementValue(presentLoad(station.load, station.stationType)), "Station Result": station.result };
        const points = station.points.filter(point => isBusinessPoint(station.stationType, point.pointNumber));
        if (!points.length) return [stationRow];
        return points.flatMap(point => {
          const pointRow = { ...stationRow, PointNumber: point.pointNumber, "Position (mm)": isUnmeasuredPoint(point) ? ""
            : exportMeasurementValue(presentPosition(point.position, station.stationType, point.pointNumber)),
            Result: displayPointResult(station, point), AttemptCount: point.attemptCount, DecideAttempt: point.decideAttempt ?? "" };
          return point.attempts.length ? point.attempts.map(attempt => ({ ...pointRow,
            AttemptNumber: attempt.attemptNumber, "PushDist (mm)": attempt.pushDist ?? "", "Bend (mm)": attempt.bend ?? "",
          })) : [pointRow];
        });
      });
    });
  }
  const headers = details ? [...exportHeaders, ...detailHeaders] : exportHeaders;
  // CSV has no text cell type. Prefix dangerous text with an apostrophe for Excel/WPS.
  // Only trusted numeric columns are exempt; never convert decimal strings to Number.
  const numericColumns = new Set(["Measurement ID", "StickNo", "RecordSeq", "TotalCount", "TIP Load (kg)", "BUTT Load (kg)",
    "Station Load (kg)", "PointNumber", "Position (mm)", "AttemptCount", "DecideAttempt", "AttemptNumber", "PushDist (mm)", "Bend (mm)"]);
  const safeRows = rows.map(row => Object.fromEntries(Object.entries(row).map(([key, value]) => [
    key,
    typeof value === "string" && !(numericColumns.has(key) && /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(value)) &&
      (/^[\s\u0000-\u001f]*[=+\-@]/.test(value) || /^[\t\r\n]/.test(value))
      ? `'${value}` : value,
  ])));
  const sheet = XLSX.utils.json_to_sheet(safeRows, {
    header: headers,
  });

  XLSX.utils.sheet_add_aoa(
    sheet,
    [headers.map(headerLabel)],
    { origin: "A1" }
  );

  // Quote every cell so CR/LF in untrusted text cannot become a new CSV row.
  const csv = XLSX.utils.sheet_to_csv(sheet, { forceQuotes: true });

  return new TextEncoder().encode(`\uFEFF${csv}`);
};

// Xuất Excel có định dạng
export const createExcelBytes = async (
  rows: ExportRow[],
  details: MeasurementDetail[]
): Promise<Uint8Array> => {
  const workbook = new ExcelJS.Workbook();
  const measurementsById = new Map(details.map(item => [item.id, item]));
  const pointsByKey = new Map(details.flatMap(item => item.stations.flatMap(station =>
    station.points.map(point => [`${item.id}:${station.stationType}:${point.pointNumber}`, { station, point }] as const))));
  const noteValue = (cell: ExcelJS.Cell, value: PresentedMeasurementValue) => {
    cell.note = measurementValueDescription(value);
  };
  // Also normalize summary rows supplied directly by callers. Details remain raw;
  // cell notes identify defaults without turning numeric cells into text labels.
  const summaryValues = rows.map(row => {
    const raw = measurementsById.get(Number(row["Measurement ID"]));
    return {
      tip: presentLoad(raw ? raw.tipLoad : row["TIP Load (kg)"], "TIP"),
      butt: presentLoad(raw ? raw.buttLoad : row["BUTT Load (kg)"], "BUTT"),
    };
  });
  rows = rows.map((row, index) => ({ ...row,
    "TIP Load (kg)": exportMeasurementValue(summaryValues[index].tip),
    "BUTT Load (kg)": exportMeasurementValue(summaryValues[index].butt),
  }));
  const realColumns = new Set([
    "TIP Load (kg)", "BUTT Load (kg)", "Station Load (kg)",
    "Position (mm)", "PushDist (mm)", "Bend (mm)",
  ]);

  workbook.creator = "Phần Mềm Thu Thập Dữ Liệu Độ Cong Gậy Golf";
  workbook.created = new Date();

  const worksheet = workbook.addWorksheet("Tổng hợp", {
    views: [
      {
        state: "frozen",
        ySplit: 1,
        showGridLines: false,
      },
    ],
  });

  // Khai báo các cột
  worksheet.columns = exportHeaders.map(header => ({
    header: headerLabel(header),
    key: header,
    width: calculateColumnWidth(rows, header),
  }));

  // Ghi dữ liệu
  rows.forEach((row, index) => {
    const added = worksheet.addRow(row);
    noteValue(added.getCell("TIP Load (kg)"), summaryValues[index].tip);
    noteValue(added.getCell("BUTT Load (kg)"), summaryValues[index].butt);
    const quality = measurementsById.get(Number(row["Measurement ID"]))?.qualityError;
    if (quality) added.getCell("Measurement ID").note = `Diagnostic dữ liệu PLC: ${quality}`;
  });

  // Apply the same formatting to every sheet, preserving the original cell values.
  const styleSheet = (worksheet: ExcelJS.Worksheet, filterable = true) => {
    // Định dạng hàng tiêu đề
    const headerRow = worksheet.getRow(1);
    headerRow.height = 32;

    headerRow.eachCell(cell => {
      cell.font = {
        name: "Calibri",
        size: 11,
        bold: true,
        color: { argb: "FFFFFFFF" },
      };

      cell.fill = {
        type: "pattern",
        pattern: "solid",
        fgColor: { argb: "FF123B5D" },
      };

      cell.alignment = {
        horizontal: "center",
        vertical: "middle",
        wrapText: true,
      };

      cell.border = {
        top: {
          style: "thin",
          color: { argb: "FFB8C9DA" },
        },
        bottom: {
          style: "thin",
          color: { argb: "FFB8C9DA" },
        },
        left: {
          style: "thin",
          color: { argb: "FFB8C9DA" },
        },
        right: {
          style: "thin",
          color: { argb: "FFB8C9DA" },
        },
      };
    });

    // Định dạng các dòng dữ liệu
    worksheet.eachRow((row, rowNumber) => {
      if (rowNumber === 1) return;

      row.height = 26;

      row.eachCell({ includeEmpty: true }, cell => {
        cell.font = {
          name: "Calibri",
          size: 11,
          color: { argb: "FF243B53" },
        };

        cell.alignment = {
          horizontal: "center",
          vertical: "middle",
          wrapText: true,
        };

        // Màu nền xen kẽ giữa các hàng
        cell.fill = {
          type: "pattern",
          pattern: "solid",
          fgColor: {
            argb: rowNumber % 2 === 0 ? "FFFFFFFF" : "FFF3F7FB",
          },
        };

        cell.border = {
          bottom: {
            style: "hair",
            color: { argb: "FFE1E8EF" },
          },
        };

        const key = worksheet.getColumn(cell.col).key;
        // PostgreSQL NUMERIC arrives as decimal text. XLSX needs numeric cells
        // for number formats/calculations; format only, without rounding values.
        // Blank/NULL/unmeasured cells and integer columns remain unchanged.
        if (realColumns.has(String(key))) {
          const original = cell.value;
          const number = typeof original === "number" ? original
            : typeof original === "string" && /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(original.trim())
              ? Number(original) : NaN;
          if (Number.isFinite(number)) {
            cell.value = number;
            cell.numFmt = "0.00";
          }
        }

        // Only result columns carry result colors, never a SKU that happens to be OK/NG.
        if (!["OverallResult", "TIP Result", "BUTT Result", "Result"].includes(String(key))) return;
        const value = String(cell.value ?? "");

        if (value === "OK") {
          cell.fill = {
            type: "pattern",
            pattern: "solid",
            fgColor: { argb: "FFE3F5EB" },
          };

          cell.font = {
            name: "Calibri",
            size: 11,
            bold: true,
            color: { argb: "FF167451" },
          };
        } else if (value === "NG") {
          cell.fill = {
            type: "pattern",
            pattern: "solid",
            fgColor: { argb: "FFFFEBEB" },
          };

          cell.font = {
            name: "Calibri",
            size: 11,
            bold: true,
            color: { argb: "FFB43F49" },
          };
        } else if (value === "SKIPPED") {
          cell.fill = { type: "pattern", pattern: "solid", fgColor: { argb: "FFEEF0F2" } };
          cell.font = { name: "Calibri", size: 11, bold: true, color: { argb: "FF627D98" } };
        }
      });
    });
    if (filterable) {
      worksheet.autoFilter = {
        from: { row: 1, column: 1 },
        to: { row: worksheet.rowCount, column: worksheet.columnCount },
      };
    }
  };
  styleSheet(worksheet);

  const tables = buildReportTables(details);
  // Join by PostgreSQL Measurement ID, never by a potentially repeated StickNo.
  const anchors = new Map(details.map(item => [item.id, { ProductCode: item.productCode, StickNo: item.clubNumber }]));

  // Compare the original rows, not merged cell values. Merge only adjacent runs;
  // station/point keys always include their parent Measurement ID.
  const mergeConsecutive = (sheet: ExcelJS.Worksheet, records: ExportRow[], groupKeys: string[], columns: string[]) => {
    for (let start = 0; start < records.length;) {
      let end = start + 1;
      while (end < records.length && groupKeys.every(key => records[end][key] === records[start][key])) end++;
      if (end - start > 1) {
        for (const key of columns) {
          const column = sheet.getColumn(key).number;
          sheet.mergeCells(start + 2, column, end + 1, column);
          sheet.getCell(start + 2, column).alignment = { horizontal: "center", vertical: "middle", wrapText: true };
        }
      }
      start = end;
    }
  };

  for (const [name, records, headers] of [
    [
      "Điểm đo",
      tables.points,
      [
        "Measurement ID",
        "ProductCode",
        "StickNo",
        "StationType",
        "PointNumber",
        "Position (mm)",
        "Result",
        "AttemptCount",
        "DecideAttempt",
      ],
    ],
    [
      "Lượt đo",
      tables.attempts,
      [
        "Measurement ID",
        "ProductCode",
        "StickNo",
        "StationType",
        "PointNumber",
        "AttemptNumber",
        "PushDist (mm)",
        "Bend (mm)",
      ],
    ],
  ] as const) {
    const sheet = workbook.addWorksheet(name, {
      views: [{ state: "frozen", ySplit: 1, showGridLines: false }],
    });
    const detailRows = records.map(record => ({ ...record, ...anchors.get(record["Measurement ID"])! }));

    sheet.columns = headers.map(header => ({
      header: headerLabel(header),
      key: header,
      width: calculateColumnWidth(detailRows, header),
    }));

    detailRows.forEach(record => {
      const added = sheet.addRow(record);
      if (name === "Điểm đo") {
        const original = pointsByKey.get(`${record["Measurement ID"]}:${record.StationType}:${record.PointNumber}`);
        if (original && !isUnmeasuredPoint(original.point)) {
          noteValue(added.getCell("Position (mm)"), presentPosition(original.point.position,
            original.station.stationType, original.point.pointNumber));
        }
      }
    });
    // Merged detail sheets are presentation views. Keep full filtering/sorting on
    // the unmerged summary; the existing CSV export retains flat detailed rows.
    styleSheet(sheet, false);
    mergeConsecutive(sheet, detailRows, ["Measurement ID"], ["Measurement ID", "ProductCode", "StickNo"]);
    mergeConsecutive(sheet, detailRows, ["Measurement ID", "StationType"], ["StationType"]);
    if (name === "Lượt đo") {
      mergeConsecutive(sheet, detailRows, ["Measurement ID", "StationType", "PointNumber"], ["PointNumber"]);
    }
    sheet.getCell("A1").note = "Sheet trình bày theo nhóm có ô gộp, không bật bộ lọc/sắp xếp để tránh sai nhóm. "
      + "Dùng sheet Tổng hợp để lọc/sắp xếp phép đo; xuất CSV nếu cần lọc từng dòng chi tiết. "
      + "Các dòng bên dưới cùng một ô gộp thuộc phép đo, trạm hoặc điểm ghi ở ô đó.";
  }

  const buffer = await workbook.xlsx.writeBuffer();

  return new Uint8Array(buffer);
};
