
import ExcelJS from "exceljs";
import * as XLSX from "xlsx";

import type { MeasurementDetail } from "../services/measurementService";
import { buildReportTables } from "./as200Report";

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
  "TIP Load (g)",
  "BUTT Result",
  "BUTT Load (g)",
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
  "TIP Load (g)": "Tải trọng TIP (g)",
  "BUTT Result": "Kết quả BUTT",
  "BUTT Load (g)": "Tải trọng BUTT (g)",
  StationType: "Trạm đo",
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
  let maxLength = header.length;

  for (const row of rows) {
    const value = String(row[header] ?? "");
    maxLength = Math.max(maxLength, value.length);
  }

  return Math.min(Math.max(maxLength + 4, 12), 45);
};

// Xuất CSV UTF-8 BOM
export const createCsvBytes = (rows: ExportRow[]): Uint8Array => {
  // CSV has no text cell type. Prefix dangerous text with an apostrophe for Excel/WPS.
  // Only trusted numeric columns are exempt; never convert decimal strings to Number.
  const numericColumns = new Set(["Measurement ID", "StickNo", "RecordSeq", "TotalCount", "TIP Load (g)", "BUTT Load (g)"]);
  const safeRows = rows.map(row => Object.fromEntries(Object.entries(row).map(([key, value]) => [
    key,
    typeof value === "string" && !(numericColumns.has(key) && /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?$/.test(value)) &&
      (/^[\s\u0000-\u001f]*[=+\-@]/.test(value) || /^[\t\r\n]/.test(value))
      ? `'${value}` : value,
  ])));
  const sheet = XLSX.utils.json_to_sheet(safeRows, {
    header: exportHeaders,
  });

  XLSX.utils.sheet_add_aoa(
    sheet,
    [exportHeaders.map(headerLabel)],
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

  workbook.creator = "Golf Measurement Report";
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
  for (const row of rows) {
    worksheet.addRow(row);
  }

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
      fgColor: { argb: "FF1769AA" },
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

      // Tô màu kết quả OK và NG
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
      }
    });
  });

  const tables = buildReportTables(details);

  for (const [name, records, headers] of [
    [
      "Điểm đo",
      tables.points,
      [
        "Measurement ID",
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
        "StationType",
        "PointNumber",
        "AttemptNumber",
        "PushDist (mm)",
        "Bend (mm)",
      ],
    ],
  ] as const) {
    const sheet = workbook.addWorksheet(name, {
      views: [{ state: "frozen", ySplit: 1 }],
    });

    sheet.columns = headers.map(header => ({
      header: headerLabel(header),
      key: header,
      width: 22,
    }));

    records.forEach(record => sheet.addRow(record));

    sheet.getRow(1).font = { bold: true };
    sheet.autoFilter = {
      from: { row: 1, column: 1 },
      to: { row: 1, column: headers.length },
    };
  }

  const buffer = await workbook.xlsx.writeBuffer();

  return new Uint8Array(buffer);
};
