import { formatReportTime, reportDateBoundary, vietnamNow } from "../../utils/reportTime";
import {
  ArrowRightOutlined,
  ReloadOutlined,
} from "@ant-design/icons";

import {
  Alert,
  Button,
  Card,
  Col,
  DatePicker,
  Empty,
  Row,
  Space,
  Spin,
  Table,
  Tag,
} from "antd";

import type { ColumnsType } from "antd/es/table";
import type { Dayjs } from "dayjs";
import { useCallback, useEffect, useRef, useState } from "react";

import {
  Cell,
  Legend,
  Pie,
  PieChart,
  Bar,
  BarChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import { useNavigate } from "react-router-dom";
import PageHeader from "../../components/PageHeader";
import "./Dashboard.css";

import {
  getDatabaseStatus,
  getMeasurementStatistics,
  getMeasurements,
  type BackendMeasurement,
  type MeasurementListRequest,
  type MeasurementStatistics,
} from "../../services/measurementService";

const initialRange: [Dayjs, Dayjs] = [
  vietnamNow().subtract(29, "day").startOf("day"),
  vietnamNow().endOf("day"),
];

const resultColors = {
  OK: "#1d9b6c",
  NG: "#d95757",
  UNMEASURED: "#829ab1",
};

function Dashboard() {
  const navigate = useNavigate();

  const [range, setRange] = useState<[Dayjs, Dayjs]>(initialRange);
  const [statistics, setStatistics] = useState<MeasurementStatistics>();
  const [recent, setRecent] = useState<BackendMeasurement[]>([]);
  const requestVersion = useRef(0);
  const hasData = useRef(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const load = useCallback(async (nextRange: [Dayjs, Dayjs], showLoading = true) => {
    const version = ++requestVersion.current;
    if (showLoading) setLoading(true);
    setError("");

    const request: MeasurementListRequest = {
      measuredFrom: reportDateBoundary(nextRange[0]),
      measuredTo: reportDateBoundary(nextRange[1], true),
    };

    try {
      const [stats, records] = await Promise.all([
        getMeasurementStatistics(request),
        getMeasurements({ ...request, page: 1, pageSize: 5 }),
      ]);

      if(version !== requestVersion.current) return;
      hasData.current = true;
      setStatistics(stats);
      setRecent(records.items);
    } catch (reason: unknown) {
      if(version !== requestVersion.current) return;
      setError(
        typeof reason === "string"
          ? reason
          : "Không thể tải dữ liệu Dashboard từ PostgreSQL."
      );
    } finally {
      if(version === requestVersion.current && showLoading) setLoading(false);
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let checking = false;
    let ready = false;

    const checkDatabase = async () => {
      if (checking) return;
      checking = true;
      try {
        const status = await getDatabaseStatus();
        if (disposed) return;
        if (status.state === "CONNECTED") {
          if (!ready) {
            ready = true;
            void load(range, !hasData.current);
          }
        } else {
          if (ready) ++requestVersion.current;
          ready = false;
          if (status.state === "CONNECTING") {
            setLoading(!hasData.current);
            setError("");
          } else {
            setLoading(false);
            setError(status.message);
          }
        }
      } catch (reason: unknown) {
        if (disposed) return;
        ++requestVersion.current;
        ready = false;
        setLoading(false);
        setError(typeof reason === "string" ? reason : "Không thể kiểm tra trạng thái cơ sở dữ liệu.");
      } finally {
        checking = false;
      }
    };

    void checkDatabase();
    const timer = window.setInterval(() => void checkDatabase(), 3000);
    return () => {
      disposed = true;
      ++requestVersion.current;
      window.clearInterval(timer);
    };
  }, [load, range]);

  const columns: ColumnsType<BackendMeasurement> = [
    {
      title: "Mã bản ghi",
      dataIndex: "id",
      align: "center",
      render: (value: number) => (
        <span style={{ fontWeight: 600, color: "#1769aa" }}>
          #{value}
        </span>
      ),
    },
    {
      title: "Mã sản phẩm",
      dataIndex: "productCode",
    },
    {
      title: "Số gậy",
      dataIndex: "clubNumber",
      align: "center",
    },
    {
      title: "Thời gian",
      dataIndex: "measuredAt",
      align: "center",
      render: (value: string) => formatReportTime(value),
    },
    {
      title: "Kết quả tổng",
      dataIndex: "overallResult",
      align: "center",
      render: (value: "OK" | "NG" | "UNMEASURED") => (
        <Tag color={value === "OK" ? "green" : value === "NG" ? "red" : "default"}>
          {value}
        </Tag>
      ),
    },
  ];

  const daily = statistics?.daily ?? [];

  const distribution = (statistics?.overallDistribution ?? []).map(
    (item) => ({
      name: item.result === "UNMEASURED" ? "Chưa đo" : item.result,
      result: item.result,
      value: item.count,
    })
  );


  return (
    <div className="dashboard-page">
      <PageHeader
        eyebrow="Tổng quan"
        title="Tổng quan kết quả đo"
        description="Theo dõi và thống kê kết quả đo gậy Golf."
        extra={
          <Space className="dashboard-header-actions" wrap>
            <DatePicker.RangePicker
              value={range}
              onChange={(value) =>
                value?.[0] &&
                value[1] &&
                setRange([value[0].startOf("day"), value[1].endOf("day")])
              }
            />

            <Button
              icon={<ReloadOutlined />}
              onClick={() => void load(range)}
            >
              Tải lại
            </Button>
          </Space>
        }
      />


      {error && (
        <Alert
          type="error"
          showIcon
          message="Không thể tải dữ liệu"
          description={error}
          style={{ marginBottom: 20 }}
        />
      )}

      {loading ? (
        <div style={{ padding: 80, textAlign: "center" }}>
          <Spin tip="Đang tải thống kê..." />
        </div>
      ) : !statistics ? (
        <Empty description="Chưa có dữ liệu thống kê." />
      ) : (
        <>
          <Row
            className="dashboard-stat-grid"
            gutter={[16, 16]}
            style={{ marginBottom: 20 }}
          >
            <Col xs={24} sm={12} lg={8} xl={4}>
              <Card className="section-card stat-card">
                <div className="stat-label">Tổng số lần đo</div>
                <div className="stat-value">
                  {statistics.totalMeasurements.toLocaleString("vi-VN")}
                </div>
              </Card>
            </Col>

            <Col xs={24} sm={12} lg={8} xl={4}>
              <Card className="section-card stat-card">
                <div className="stat-label">Tổng OK</div>
                <div className="stat-value" style={{ color: "#1d9b6c" }}>
                  {statistics.totalOk.toLocaleString("vi-VN")}
                </div>
              </Card>
            </Col>

            <Col xs={24} sm={12} lg={8} xl={4}>
              <Card className="section-card stat-card">
                <div className="stat-label">Tổng NG</div>
                <div className="stat-value" style={{ color: "#d95757" }}>
                  {statistics.totalNg.toLocaleString("vi-VN")}
                </div>
              </Card>
            </Col>

            <Col xs={24} sm={12} lg={8} xl={4}>
              <Card className="section-card stat-card">
                <div className="stat-label">Tỷ lệ đạt</div>
                <div className="stat-value" style={{ color: "#1d9b6c" }}>
                  {statistics.passRate.toLocaleString("vi-VN", {
                    maximumFractionDigits: 1,
                  })}%
                </div>
              </Card>
            </Col>

            <Col xs={24} sm={12} lg={8} xl={4}>
              <Card className="section-card stat-card">
                <div className="stat-label">Tip NG</div>
                <div className="stat-value" style={{ color: "#d95757" }}>
                  {statistics.tipNg.toLocaleString("vi-VN")}
                </div>
              </Card>
            </Col>

            <Col xs={24} sm={12} lg={8} xl={4}>
              <Card className="section-card stat-card">
                <div className="stat-label">Butt NG</div>
                <div className="stat-value" style={{ color: "#d95757" }}>
                  {statistics.buttNg.toLocaleString("vi-VN")}
                </div>
              </Card>
            </Col>
          </Row>


          <div className="content-grid">
            <Card className="section-card dashboard-chart-card">
              <div className="card-heading">
                <div>
                  <div className="card-title">
                    Số lần đo theo ngày
                  </div>

                  <div className="card-caption">
                    Tổng số, OK và NG
                  </div>
                </div>
              </div>

              <div className="chart-wrap">
                <ResponsiveContainer width="100%" height="100%">
                  <BarChart data={daily}>
                    <CartesianGrid
                      stroke="#edf2f7"
                      vertical={false}
                    />

                    <XAxis
                      dataKey="date"
                      tick={{ fill: "#627d98", fontSize: 13 }}
                    />

                    <YAxis
                      allowDecimals={false}
                      tick={{ fill: "#627d98", fontSize: 13 }}
                    />

                    <Tooltip contentStyle={{ fontSize: 15 }} />
                    <Legend wrapperStyle={{ fontSize: 14 }} />

                    <Bar
                      dataKey="total"
                      name="Tổng"
                      fill="#2f80c1"
                      radius={[4, 4, 0, 0]}
                    />

                    <Bar
                      dataKey="ok"
                      name="OK"
                      fill="#1d9b6c"
                      radius={[4, 4, 0, 0]}
                    />

                    <Bar
                      dataKey="ng"
                      name="NG"
                      fill="#d95757"
                      radius={[4, 4, 0, 0]}
                    />
                  </BarChart>
                </ResponsiveContainer>
              </div>
            </Card>

            <Card className="section-card dashboard-chart-card">
              <div className="card-heading">
                <div>
                  <div className="card-title">
                    Phân bố kết quả tổng
                  </div>

                  <div className="card-caption">
                    Theo khoảng thời gian đã chọn
                  </div>
                </div>
              </div>

              {distribution.length === 0 ? (
                <Empty description="Chưa có dữ liệu" />
              ) : (
                <div className="chart-wrap">
                  <ResponsiveContainer width="100%" height="100%">
                    <PieChart>
                      <Pie
                        data={distribution}
                        dataKey="value"
                        nameKey="name"
                        cx="50%"
                        cy="50%"
                        innerRadius={55}
                        outerRadius={88}
                        label={({ name, value }) => `${name}: ${value}`}
                      >
                        {distribution.map((item) => (
                          <Cell
                            key={item.name}
                            fill={
                              resultColors[
                                item.result as keyof typeof resultColors
                              ] ?? resultColors.UNMEASURED
                            }
                          />
                        ))}
                      </Pie>

                      <Tooltip contentStyle={{ fontSize: 15 }} />
                      <Legend wrapperStyle={{ fontSize: 14 }} />
                    </PieChart>
                  </ResponsiveContainer>
                </div>
              )}
            </Card>
          </div>

          <Card
            className="section-card"
            style={{ marginTop: 20 }}
          >
            <div className="card-heading">
              <div>
                <div className="card-title">
                  Kết quả đo gần đây
                </div>

                <div className="card-caption">
                  Các kết quả đo mới nhất trong khoảng thời gian đã chọn.
                </div>
              </div>

              <Button
                type="link"
                onClick={() => navigate("/measurements")}
              >
                Xem lịch sử <ArrowRightOutlined />
              </Button>
            </div>

            {recent.length === 0 ? (
              <Empty description="Chưa có phép đo trong khoảng thời gian này." />
            ) : (
              <Table
                className="dashboard-recent-table"
                rowKey="id"
                columns={columns}
                dataSource={recent}
                pagination={false}
                scroll={{ x: 780 }}
                onRow={(record) => ({
                  onClick: () => navigate(`/measurements/${record.id}`),
                  style: { cursor: "pointer" },
                })}
              />
            )}
          </Card>
        </>
      )}
    </div>
  );
}

export default Dashboard;
