import { formatReportTime, reportDateBoundary, vietnamNow } from "../../utils/reportTime";
import {
  FilterOutlined,
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
import type { Dayjs } from "dayjs";
import { save } from "@tauri-apps/plugin-dialog";
import { writeFile } from "@tauri-apps/plugin-fs";
import { useCallback, useEffect, useRef, useState } from "react";

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
import "./Reports.css";

import {
  getMeasurementStatistics,
  getMeasurementReport,
  getMeasurements,
  type BackendMeasurement,
  type MeasurementListRequest,
  type MeasurementStatistics,
} from "../../services/measurementService";

import { createCsvBytes, createExcelBytes } from "../../utils/as200Export";
import { buildReportTables, formatDecimal } from "../../utils/as200Report";

type ResultFilter = "OK" | "NG" | "UNMEASURED" | undefined;
type ExportFormat = "csv" | "xlsx";
const createDefaultRange = (): [Dayjs, Dayjs] => [
  vietnamNow().subtract(29, "day").startOf("day"),
  vietnamNow().endOf("day"),
];

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
      measuredFrom: reportDateBoundary(range[0]),
      measuredTo: reportDateBoundary(range[1], true),
    };
  });

  const [stats, setStats] = useState<MeasurementStatistics>();
  const [records, setRecords] = useState<BackendMeasurement[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);

  const requestVersion = useRef(0);
  const [loading, setLoading] = useState(true);
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState("");

  // Tải thống kê và danh sách từ PostgreSQL
  const load = useCallback(
    async (
      nextFilters: MeasurementListRequest,
      nextPage = 1
    ) => {
      const version = ++requestVersion.current;
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

        if(version !== requestVersion.current) return;
        setStats(summary);
        setRecords(response.items);
        setTotal(response.total);
        setPage(response.page);
      } catch (reason: unknown) {
        if(version !== requestVersion.current) return;
        setError(
          typeof reason === "string"
            ? reason
            : "Không thể tải dữ liệu báo cáo từ PostgreSQL."
        );

        setStats(undefined);
        setRecords([]);
        setTotal(0);
      } finally {
        if(version === requestVersion.current) setLoading(false);
      }
    },
    []
  );

  useEffect(() => {
    void load(filters);
  }, [filters, load]);

  // Áp dụng bộ lọc
  const applyFilters = () => {
    const club = draftClub.trim() ? Number(draftClub) : undefined;
    if (club !== undefined && (!Number.isInteger(club) || club < -2147483648 || club > 2147483647)) {
      messageApi.error("Số gậy phải là số nguyên signed 32-bit theo AS200.");
      return;
    }
    setFilters({
      measuredFrom: reportDateBoundary(draftRange[0]),
      measuredTo: reportDateBoundary(draftRange[1], true),
      productCode: draftProduct || undefined,
      clubNumber: club,
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
      measuredFrom: reportDateBoundary(range[0]),
      measuredTo: reportDateBoundary(range[1], true),
    });
  };

  // Xuất Excel hoặc CSV
  const exportReport = async (format: ExportFormat) => {
    if (exporting) return;

    setExporting(true);

    try {
      const all = await getMeasurementReport(filters);

      if (all.length === 0) {
        messageApi.info("Không có dữ liệu phù hợp để xuất.");
        return;
      }

      const rows = buildReportTables(all).measurements;

      const suggested =
        `bao-cao-do-gay-${vietnamNow().format("YYYYMMDD-HHmmss")}.${format}`;

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
        bytes = await createExcelBytes(rows, all);
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
      align: "center",
      width: 96,
      render: (value: number) => `#${value}`,
    },
    {
      title: "Thời gian nhận",
      dataIndex: "measuredAt",
      align: "center",
      width: 165,
      render: (value: string) =>
        formatReportTime(value),
    },
    {
      title: "Mã sản phẩm",
      dataIndex: "productCode",
      width: 165,
    },
    {
      title: "Số gậy",
      dataIndex: "clubNumber",
      align: "center",
      width: 75,
    },
    {
      title: "RecordSeq", dataIndex: "recordSeq", align: "center", width: 88,
    },
    {title:"TIP Load (g)",dataIndex:"tipLoad",render:formatDecimal,align:"right",width:110},
    {title:"BUTT Load (g)",dataIndex:"buttLoad",render:formatDecimal,align:"right",width:110},
    {
      title: "Tip",
      dataIndex: "tipResult",
      align: "center",
      width: 70,
      render: (value: string) => (
        <Tag color={value === "OK" ? "green" : value === "NG" ? "red" : "default"}>
          {value}
        </Tag>
      ),
    },
    {
      title: "Butt",
      dataIndex: "buttResult",
      align: "center",
      width: 70,
      render: (value: string) => (
        <Tag color={value === "OK" ? "green" : value === "NG" ? "red" : "default"}>
          {value}
        </Tag>
      ),
    },
    {
      title: "Kết quả tổng",
      dataIndex: "overallResult",
      align: "center",
      width: 110,
      render: (value: string) => (
        <Tag color={value === "OK" ? "green" : value === "NG" ? "red" : "default"}>
          {value}
        </Tag>
      ),
    },
  ];

  const station = stats?.stationResults ?? [];

  return (
    <div className="reports-page">
      {contextHolder}

      <PageHeader
        eyebrow="Phân tích & xuất dữ liệu"
        title="Báo cáo kết quả đo"
        description="Thống kê, phân tích và xuất báo cáo kết quả đo gậy Golf."
        extra={
          <Space className="reports-header-actions" wrap>
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

        <div className="reports-filter-row">
          <div className="reports-filter-field">
            <DatePicker.RangePicker
              style={{ width: "100%" }}
              value={draftRange}
              onChange={(value) => {
                if (value?.[0] && value[1]) {
                  setDraftRange([value[0].startOf("day"), value[1].endOf("day")]);
                }
              }}
            />
          </div>

          <div className="reports-filter-field">
            <Input
              value={draftProduct}
              onChange={(event) =>
                setDraftProduct(event.target.value)
              }
              placeholder="Mã sản phẩm"
            />
          </div>

          <div className="reports-filter-field">
            <Input
              value={draftClub}
              onChange={(event) =>
                setDraftClub(event.target.value)
              }
              placeholder="Số thứ tự gậy"
            />
          </div>

          {(
            [
              ["Tip", draftTip, setDraftTip],
              ["Butt", draftButt, setDraftButt],
              ["Kết quả tổng thể", draftOverall, setDraftOverall],
            ] as const
          ).map(([label, value, setter]) => (
            <div className="reports-filter-field" key={label}>
              <Select
                allowClear
                style={{ width: "100%" }}
                placeholder={label}
                value={value}
                onChange={(nextValue) => setter(nextValue)}
                options={[
                  { value: "OK", label: "OK" },
                  { value: "NG", label: "NG" },
                  { value: "UNMEASURED", label: "Chưa đo" },
                ]}
              />
            </div>
          ))}
        </div>
        <div className="reports-filter-actions">
          <Button type="primary" icon={<FilterOutlined />} onClick={applyFilters}>
            Áp dụng
          </Button>
          <Button icon={<ReloadOutlined />} onClick={reset}>
            Đặt lại
          </Button>
        </div>
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
                  md={8}
                  xl={4}
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

                    <XAxis dataKey="date" tick={{ fill: "#627d98", fontSize: 13 }} />
                    <YAxis allowDecimals={false} tick={{ fill: "#627d98", fontSize: 13 }} />
                    <Tooltip contentStyle={{ fontSize: 15 }} />
                    <Legend wrapperStyle={{ fontSize: 14 }} />

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

                    <XAxis dataKey="station" tick={{ fill: "#627d98", fontSize: 13 }} />
                    <YAxis allowDecimals={false} tick={{ fill: "#627d98", fontSize: 13 }} />
                    <Tooltip contentStyle={{ fontSize: 15 }} />
                    <Legend wrapperStyle={{ fontSize: 14 }} />

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
                  className="reports-table"
                  rowKey="id"
                  columns={columns}
                  dataSource={records}
                  scroll={{ x: 1059 }}
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
    </div>
  );
}

export default Reports;
