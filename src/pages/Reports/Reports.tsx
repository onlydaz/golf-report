
import {
  DownloadOutlined,
  FileExcelOutlined,
  FileTextOutlined,
  ReloadOutlined,
} from "@ant-design/icons";

import {
  Alert,
  Button,
  Card,
  Col,
  DatePicker,
  Empty,
  Input,
  Row,
  Select,
  Space,
  Spin,
  Statistic,
  Table,
  Tag,
  message,
} from "antd";

import type { ColumnsType, TablePaginationConfig } from "antd/es/table";
import dayjs, { type Dayjs } from "dayjs";
import { save } from "@tauri-apps/plugin-dialog";
import { writeFile } from "@tauri-apps/plugin-fs";
import ExcelJS from "exceljs";
import * as XLSX from "xlsx";
import { useCallback, useEffect, useState } from "react";

import {
  Bar,
  BarChart,
  CartesianGrid,
  Legend,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import { useNavigate } from "react-router-dom";
import PageHeader from "../../components/PageHeader";

import {
  getMeasurementStatistics,
  getMeasurements,
  type BackendMeasurement,
  type MeasurementListRequest,
  type MeasurementStatistics,
} from "../../services/measurementService";

type ResultFilter = "OK" | "NG" | undefined;
type ExportFormat = "csv" | "xlsx";
type ExportValue = string | number;
type ExportRow = Record<string, ExportValue>;

const createDefaultRange = (): [Dayjs, Dayjs] => [
  dayjs().subtract(29, "day").startOf("day"),
  dayjs().endOf("day"),
];

// Thứ tự và tên các cột khi xuất báo cáo
const exportHeaders = [
  "Mã bản ghi",
  "Thời gian đo",
  "Mã sản phẩm",
  "Số thứ tự gậy",
  "Weight",
  "Tip",
  "Butt",
  "Kết quả tổng",

  "Tip 1 (mm)",
  "Tip 1 - Đo lại",
  "Tip 2 (mm)",
  "Tip 2 - Đo lại",
  "Tip 3 (mm)",
  "Tip 3 - Đo lại",
  "Tip 4 (mm)",
  "Tip 4 - Đo lại",

  "Butt (mm)",
];

// Chuyển dữ liệu PostgreSQL thành dòng báo cáo
const buildExportRows = (
  measurements: BackendMeasurement[]
): ExportRow[] => {
  return measurements.map((item) => {
    const row: ExportRow = {
      "Mã bản ghi": item.id,
      "Thời gian đo": new Date(item.measuredAt).toLocaleString("vi-VN"),
      "Mã sản phẩm": item.productCode,
      "Số thứ tự gậy": item.clubNumber,
      Weight: item.weight,
      Tip: item.tipResult,
      Butt: item.buttResult,
      "Kết quả tổng": item.overallResult,
    };

    for (let position = 1; position <= 4; position++) {
      const tip = item.tipPositions?.find(
        (value) => value.positionNumber === position
      );

      row[`Tip ${position} (mm)`] = tip?.finalDistance ?? "";
      row[`Tip ${position} - Đo lại`] = tip?.retryCount ?? "";
    }

    row["Butt (mm)"] = item.buttDistance ?? "";

    return row;
  });
};

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
const createCsvBytes = (rows: ExportRow[]): Uint8Array => {
  const sheet = XLSX.utils.json_to_sheet(rows, {
    header: exportHeaders,
  });

  const csv = XLSX.utils.sheet_to_csv(sheet);

  return new TextEncoder().encode(`\uFEFF${csv}`);
};

// Xuất Excel có định dạng
const createExcelBytes = async (
  rows: ExportRow[]
): Promise<Uint8Array> => {
  const workbook = new ExcelJS.Workbook();

  workbook.creator = "Golf Measurement Report";
  workbook.created = new Date();

  const worksheet = workbook.addWorksheet("Báo cáo", {
    views: [
      {
        state: "frozen",
        ySplit: 1,
        showGridLines: false,
      },
    ],
  });

  // Khai báo các cột
  worksheet.columns = exportHeaders.map((header) => ({
    header,
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

  headerRow.eachCell((cell) => {
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

    row.eachCell({ includeEmpty: true }, (cell) => {
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
          argb:
            rowNumber % 2 === 0
              ? "FFFFFFFF"
              : "FFF3F7FB",
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

  const buffer = await workbook.xlsx.writeBuffer();

  return new Uint8Array(buffer);
};

function Reports() {
  const navigate = useNavigate();
  const [messageApi, contextHolder] = message.useMessage();

  const [draftRange, setDraftRange] =
    useState<[Dayjs, Dayjs]>(createDefaultRange);

  const [draftProduct, setDraftProduct] = useState("");
  const [draftClub, setDraftClub] = useState("");

  const [draftTip, setDraftTip] = useState<ResultFilter>();
  const [draftButt, setDraftButt] = useState<ResultFilter>();
  const [draftOverall, setDraftOverall] = useState<ResultFilter>();

  const [filters, setFilters] = useState<MeasurementListRequest>(() => {
    const range = createDefaultRange();

    return {
      measuredFrom: range[0].toISOString(),
      measuredTo: range[1].toISOString(),
    };
  });

  const [stats, setStats] = useState<MeasurementStatistics>();
  const [records, setRecords] = useState<BackendMeasurement[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);

  const [loading, setLoading] = useState(true);
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState("");

  // Tải thống kê và danh sách từ PostgreSQL
  const load = useCallback(
    async (
      nextFilters: MeasurementListRequest,
      nextPage = 1
    ) => {
      setLoading(true);
      setError("");

      try {
        const request = {
          ...nextFilters,
          page: nextPage,
          pageSize: 10,
        };

        const [summary, response] = await Promise.all([
          getMeasurementStatistics(nextFilters),
          getMeasurements(request),
        ]);

        setStats(summary);
        setRecords(response.items);
        setTotal(response.total);
        setPage(response.page);
      } catch (reason: unknown) {
        setError(
          typeof reason === "string"
            ? reason
            : "Không thể tải dữ liệu báo cáo từ PostgreSQL."
        );

        setStats(undefined);
        setRecords([]);
        setTotal(0);
      } finally {
        setLoading(false);
      }
    },
    []
  );

  useEffect(() => {
    void load(filters);
  }, [filters, load]);

  // Áp dụng bộ lọc
  const applyFilters = () => {
    setFilters({
      measuredFrom: draftRange[0].toISOString(),
      measuredTo: draftRange[1].toISOString(),
      productCode: draftProduct || undefined,
      clubNumber: draftClub || undefined,
      tipResult: draftTip,
      buttResult: draftButt,
      result: draftOverall,
    });
  };

  // Đặt lại bộ lọc
  const reset = () => {
    const range = createDefaultRange();

    setDraftRange(range);
    setDraftProduct("");
    setDraftClub("");
    setDraftTip(undefined);
    setDraftButt(undefined);
    setDraftOverall(undefined);

    setFilters({
      measuredFrom: range[0].toISOString(),
      measuredTo: range[1].toISOString(),
    });
  };

  // Lấy toàn bộ dữ liệu theo bộ lọc để xuất file
  const exportRows = async (): Promise<BackendMeasurement[]> => {
    const all: BackendMeasurement[] = [];

    let current = 1;
    let count = 0;

    do {
      const response = await getMeasurements({
        ...filters,
        page: current,
        pageSize: 100,
      });

      all.push(...response.items);
      count = response.total;
      current += 1;

      if (response.items.length === 0) break;
    } while (all.length < count);

    return all;
  };

  // Xuất Excel hoặc CSV
  const exportReport = async (format: ExportFormat) => {
    if (exporting) return;

    setExporting(true);

    try {
      const all = await exportRows();

      if (all.length === 0) {
        messageApi.info("Không có dữ liệu phù hợp để xuất.");
        return;
      }

      const rows = buildExportRows(all);

      const suggested =
        `bao-cao-do-gay-${dayjs().format("YYYYMMDD-HHmmss")}.${format}`;

      const path = await save({
        defaultPath: suggested,
        filters: [
          {
            name: format === "csv" ? "CSV" : "Excel",
            extensions: [format],
          },
        ],
      });

      if (!path) return;

      let bytes: Uint8Array;

      if (format === "csv") {
        bytes = createCsvBytes(rows);
      } else {
        bytes = await createExcelBytes(rows);
      }

      await writeFile(path, bytes);

      messageApi.success(
        `Đã xuất thành công ${all.length} bản ghi ra ${format === "csv" ? "CSV" : "Excel"}.`
      );
    } catch (reason: unknown) {
      messageApi.error(
        typeof reason === "string"
          ? reason
          : "Xuất báo cáo thất bại."
      );
    } finally {
      setExporting(false);
    }
  };

  // Cột bảng dữ liệu Reports
  const columns: ColumnsType<BackendMeasurement> = [
    {
      title: "Mã bản ghi",
      dataIndex: "id",
      render: (value: number) => `#${value}`,
    },
    {
      title: "Thời gian đo",
      dataIndex: "measuredAt",
      render: (value: string) =>
        new Date(value).toLocaleString("vi-VN"),
    },
    {
      title: "Mã sản phẩm",
      dataIndex: "productCode",
    },
    {
      title: "Số gậy",
      dataIndex: "clubNumber",
    },
    {
      title: "Weight",
      dataIndex: "weight",
    },
    {
      title: "Tip",
      dataIndex: "tipResult",
      render: (value: string) => (
        <Tag color={value === "OK" ? "green" : "red"}>
          {value}
        </Tag>
      ),
    },
    {
      title: "Butt",
      dataIndex: "buttResult",
      render: (value: string) => (
        <Tag color={value === "OK" ? "green" : "red"}>
          {value}
        </Tag>
      ),
    },
    {
      title: "Kết quả tổng",
      dataIndex: "overallResult",
      render: (value: string) => (
        <Tag color={value === "OK" ? "green" : "red"}>
          {value}
        </Tag>
      ),
    },
  ];

  const station = stats?.stationResults ?? [];

  return (
    <>
      {contextHolder}

      <PageHeader
        eyebrow="Phân tích & xuất dữ liệu"
        title="Báo cáo kết quả đo"
        description="Thống kê, phân tích và xuất báo cáo kết quả đo gậy Golf."
        extra={
          <Space>
            <Button
              icon={<FileTextOutlined />}
              loading={exporting}
              disabled={exporting}
              onClick={() => void exportReport("csv")}
            >
              Xuất CSV
            </Button>

            <Button
              type="primary"
              icon={<FileExcelOutlined />}
              loading={exporting}
              disabled={exporting}
              onClick={() => void exportReport("xlsx")}
            >
              Xuất Excel
            </Button>
          </Space>
        }
      />

      {error && (
        <Alert
          type="error"
          showIcon
          message="Không thể tải báo cáo"
          description={error}
          style={{ marginBottom: 20 }}
        />
      )}

      {/* Bộ lọc báo cáo */}
      <Card
        className="section-card"
        style={{ marginBottom: 20 }}
      >
        <div className="card-heading">
          <div>
            <div className="card-title">
              Bộ lọc báo cáo
            </div>
            <div className="card-caption">
              Lọc dữ liệu và cập nhật thống kê
            </div>
          </div>
        </div>

        <Row gutter={[12, 12]}>
          <Col xs={24} md={8}>
            <DatePicker.RangePicker
              style={{ width: "100%" }}
              value={draftRange}
              onChange={(value) => {
                if (value?.[0] && value[1]) {
                  setDraftRange([value[0], value[1]]);
                }
              }}
            />
          </Col>

          <Col xs={12} md={4}>
            <Input
              value={draftProduct}
              onChange={(event) =>
                setDraftProduct(event.target.value)
              }
              placeholder="Mã sản phẩm"
            />
          </Col>

          <Col xs={12} md={4}>
            <Input
              value={draftClub}
              onChange={(event) =>
                setDraftClub(event.target.value)
              }
              placeholder="Số thứ tự gậy"
            />
          </Col>

          {(
            [
              ["Tip", draftTip, setDraftTip],
              ["Butt", draftButt, setDraftButt],
              ["Kết quả tổng", draftOverall, setDraftOverall],
            ] as const
          ).map(([label, value, setter]) => (
            <Col xs={8} md={2} key={label}>
              <Select
                allowClear
                style={{ width: "100%" }}
                placeholder={label}
                value={value}
                onChange={(nextValue) => setter(nextValue)}
                options={[
                  { value: "OK", label: "OK" },
                  { value: "NG", label: "NG" },
                ]}
              />
            </Col>
          ))}

          <Col xs={24} md={4}>
            <Space>
              <Button
                type="primary"
                icon={<DownloadOutlined />}
                onClick={applyFilters}
              >
                Áp dụng
              </Button>

              <Button
                icon={<ReloadOutlined />}
                onClick={reset}
              >
                Đặt lại
              </Button>
            </Space>
          </Col>
        </Row>
      </Card>

      {loading ? (
        <div
          style={{
            padding: 60,
            textAlign: "center",
          }}
        >
          <Spin tip="Đang tải báo cáo..." />
        </div>
      ) : (
        stats && (
          <>
            {/* Các chỉ số thống kê */}
            <Row
              className="report-stat-grid"
              gutter={[16, 16]}
              style={{ marginBottom: 20 }}
            >
              {[
                ["Tổng số lần đo", stats.totalMeasurements],
                ["Tổng OK", stats.totalOk],
                ["Tổng NG", stats.totalNg],
                [
                  "Tỷ lệ đạt",
                  `${stats.passRate.toLocaleString("vi-VN", {
                    maximumFractionDigits: 1,
                  })}%`,
                ],
                ["Tip NG", stats.tipNg],
                ["Butt NG", stats.buttNg],
              ].map(([title, value]) => (
                <Col
                  xs={12}
                  md={4}
                  key={title as string}
                >
                  <Card className="section-card">
                    <Statistic
                      title={title as string}
                      value={value}
                    />
                  </Card>
                </Col>
              ))}
            </Row>

            {/* Biểu đồ theo ngày và phân bố */}
            <Card className="section-card">
              <div className="card-title">
                Số lần đo theo ngày
              </div>

              <div className="chart-wrap">
                <ResponsiveContainer
                  width="100%"
                  height="100%"
                >
                  <BarChart data={stats.daily}>
                    <CartesianGrid
                      stroke="#edf2f7"
                      vertical={false}
                    />

                    <XAxis dataKey="date" />
                    <YAxis allowDecimals={false} />
                    <Tooltip />
                    <Legend />

                    <Bar
                      dataKey="ok"
                      name="OK"
                      fill="#1d9b6c"
                    />

                    <Bar
                      dataKey="ng"
                      name="NG"
                      fill="#d95757"
                    />
                  </BarChart>
                </ResponsiveContainer>
              </div>
            </Card>

            {/* So sánh Tip và Butt */}
            <Card
              className="section-card"
              style={{ marginTop: 20 }}
            >
              <div className="card-title">
                So sánh Tip và Butt
              </div>

              <div className="chart-wrap">
                <ResponsiveContainer
                  width="100%"
                  height="100%"
                >
                  <BarChart data={station}>
                    <CartesianGrid
                      stroke="#edf2f7"
                      vertical={false}
                    />

                    <XAxis dataKey="station" />
                    <YAxis allowDecimals={false} />
                    <Tooltip />
                    <Legend />

                    <Bar
                      dataKey="ok"
                      name="OK"
                      fill="#1d9b6c"
                    />

                    <Bar
                      dataKey="ng"
                      name="NG"
                      fill="#d95757"
                    />
                  </BarChart>
                </ResponsiveContainer>
              </div>
            </Card>

            {/* Bảng dữ liệu báo cáo */}
            <Card
              className="section-card"
              style={{ marginTop: 20 }}
            >
              <div className="card-heading">
                <div>
                  <div className="card-title">
                    Bảng dữ liệu báo cáo
                  </div>

                  <div className="card-caption">
                    Tổng cộng{" "}
                    {total.toLocaleString("vi-VN")}{" "}
                    bản ghi phù hợp
                  </div>
                </div>
              </div>

              {records.length === 0 ? (
                <Empty description="Không có dữ liệu phù hợp." />
              ) : (
                <Table
                  rowKey="id"
                  columns={columns}
                  dataSource={records}
                  pagination={{
                    current: page,
                    pageSize: 10,
                    total,
                    showSizeChanger: false,
                  } as TablePaginationConfig}
                  onChange={(pagination) =>
                    void load(
                      filters,
                      pagination.current ?? 1
                    )
                  }
                  onRow={(record) => ({
                    onClick: () =>
                      navigate(
                        `/measurements/${record.id}`
                      ),
                    style: {
                      cursor: "pointer",
                    },
                  })}
                />
              )}
            </Card>
          </>
        )
      )}
    </>
  );
}

export default Reports;
