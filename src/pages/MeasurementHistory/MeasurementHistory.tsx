import { formatReportTime, reportDateBoundary } from "../../utils/reportTime";
import { EyeOutlined, ReloadOutlined, SearchOutlined } from "@ant-design/icons";
import { Alert, Button, Card, DatePicker, Empty, Input, Select, Spin, Table } from "antd";
import type { ColumnsType, TablePaginationConfig } from "antd/es/table";
import type { Dayjs } from "dayjs";
import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";

import { ResultTag } from "../../components/measurement/MeasurementDetails";
import PageHeader from "../../components/PageHeader";
import "./MeasurementHistory.css";
import {
  getMeasurements,
  type BackendMeasurement,
  type MeasurementListRequest,
} from "../../services/measurementService";

type Result = "OK" | "NG" | "UNMEASURED";

function MeasurementHistory() {
  const navigate = useNavigate();

  const [search, setSearch] = useState("");
  const [overallResult, setOverallResult] = useState<Result>();
  const [tipResult, setTipResult] = useState<Result>();
  const [buttResult, setButtResult] = useState<Result>();
  const [dateRange, setDateRange] = useState<[Dayjs, Dayjs]>();
  const [data, setData] = useState<BackendMeasurement[]>([]);
  const [total, setTotal] = useState(0);
  const dataRef = useRef<BackendMeasurement[]>([]);
  const totalRef = useRef(0);
  const [page, setPage] = useState(1);
  const requestVersion = useRef(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const loadMeasurements = useCallback(async (request: MeasurementListRequest, background = false) => {
    const version = ++requestVersion.current;
    if (!background) setLoading(true);

    try {
      const response = await getMeasurements(request);
      if(version !== requestVersion.current) return;
      if (JSON.stringify(dataRef.current) !== JSON.stringify(response.items)) {
        dataRef.current = response.items;
        setData(response.items);
      }
      if (totalRef.current !== response.total) {
        totalRef.current = response.total;
        setTotal(response.total);
      }
      setError("");
    } catch (reason: unknown) {
      if(version !== requestVersion.current) return;
      if (!background) {
        dataRef.current = [];
        totalRef.current = 0;
        setData([]);
        setTotal(0);
      }
      setError(
        typeof reason === "string"
          ? reason
          : "Không thể tải lịch sử đo từ cơ sở dữ liệu."
      );
    } finally {
      if(!background && version === requestVersion.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let refresh: ReturnType<typeof setTimeout>;
    ++requestVersion.current;
    setLoading(true);
    setError("");
    let background = false;
    const poll = async () => {
      await loadMeasurements({
        page, pageSize: 10, search: search.trim() || undefined,
        result: overallResult, tipResult, buttResult,
        measuredFrom: dateRange ? reportDateBoundary(dateRange[0]) : undefined,
        measuredTo: dateRange ? reportDateBoundary(dateRange[1], true) : undefined,
      }, background);
      background = true;
      if (!disposed) refresh = setTimeout(() => void poll(), 3000);
    };
    const timer = setTimeout(() => void poll(), 250);
    return () => { disposed = true; ++requestVersion.current; clearTimeout(timer); clearTimeout(refresh); };
  }, [buttResult, dateRange, loadMeasurements, overallResult, page, search, tipResult]);

  const clearFilters = () => {
    setSearch("");
    setOverallResult(undefined);
    setTipResult(undefined);
    setButtResult(undefined);
    setDateRange(undefined);
    setPage(1);
  };

  const columns: ColumnsType<BackendMeasurement> = [
    {
      title: "Mã bản ghi",
      dataIndex: "id",
      align: "center",
      width: 96,
      render: (value: number) => (
        <span style={{ fontWeight: 600, color: "#1769aa" }}>
          #{value}
        </span>
      ),
    },
    {
      title: "Mã sản phẩm",
      dataIndex: "productCode",
      width: 176,
    },
    {
      title: "Số thứ tự gậy",
      dataIndex: "clubNumber",
      align: "center",
      width: 120,
    },
    {
      title: "Thời gian nhận",
      dataIndex: "measuredAt",
      align: "center",
      width: 172,
      render: (value: string) => formatReportTime(value),
    },
    {
      title: "RecordSeq", dataIndex: "recordSeq", width: 94, align: "center",
    },
    {
      title: "Kết quả Tip",
      dataIndex: "tipResult",
      align: "center",
      width: 102,
      render: (value: Result) => <ResultTag result={value} />,
    },
    {
      title: "Kết quả Butt",
      dataIndex: "buttResult",
      align: "center",
      width: 104,
      render: (value: Result) => <ResultTag result={value} />,
    },
    {
      title: "Kết quả tổng thể",
      dataIndex: "overallResult",
      align: "center",
      width: 130,
      render: (value: Result) => <ResultTag result={value} />,
    },
    {
      title: "",
      key: "action",
      align: "center",
      width: 142,
      render: (_, record) => (
        <Button
          type="link"
          icon={<EyeOutlined />}
          onClick={() => navigate(`/measurements/${record.id}`)}
        >
          Xem chi tiết
        </Button>
      ),
    },
  ];

  const pagination: TablePaginationConfig = {
    current: page,
    pageSize: 10,
    total,
    showSizeChanger: false,
    showTotal: (value, range) => `${range[0]}-${range[1]} trong ${value} bản ghi`,
    onChange: (nextPage) => setPage(nextPage),
  };

  return (
    <div className="measurement-history-page">
      <PageHeader
        eyebrow="Bản ghi"
        title="Lịch sử đo"
        description="Tra cứu và xem chi tiết các kết quả đo đã ghi nhận."
      />

      <Card className="section-card">
        <div className="history-filter-row">
          <div className="filter-field filter-field-wide">
            <span className="filter-label">
              Mã sản phẩm hoặc số gậy
            </span>

            <Input
              prefix={<SearchOutlined style={{ color: "#9fb3c8" }} />}
              placeholder="Nhập nội dung tìm kiếm..."
              value={search}
              onChange={(event) => {
                setSearch(event.target.value);
                setPage(1);
              }}
              allowClear
            />
          </div>
          <div className="filter-field">
            <span className="filter-label">Kết quả tổng thể</span>

            <Select
              placeholder="Tất cả"
              value={overallResult}
              onChange={(value) => {
                setOverallResult(value);
                setPage(1);
              }}
              allowClear
              style={{ width: "100%" }}
              options={[
                { value: "OK", label: "OK" },
                { value: "NG", label: "NG" },
                { value: "UNMEASURED", label: "Chưa đo" },
              ]}
            />
          </div>

          <div className="filter-field">
            <span className="filter-label">Kết quả Tip</span>

            <Select
              placeholder="Tất cả"
              value={tipResult}
              onChange={(value) => {
                setTipResult(value);
                setPage(1);
              }}
              allowClear
              style={{ width: "100%" }}
              options={[
                { value: "OK", label: "OK" },
                { value: "NG", label: "NG" },
                { value: "UNMEASURED", label: "Chưa đo" },
              ]}
            />
          </div>

          <div className="filter-field">
            <span className="filter-label">Kết quả Butt</span>

            <Select
              placeholder="Tất cả"
              value={buttResult}
              onChange={(value) => {
                setButtResult(value);
                setPage(1);
              }}
              allowClear
              style={{ width: "100%" }}
              options={[
                { value: "OK", label: "OK" },
                { value: "NG", label: "NG" },
                { value: "UNMEASURED", label: "Chưa đo" },
              ]}
            />
          </div>

          <div className="filter-field">
            <span className="filter-label">Khoảng thời gian đo</span>

            <DatePicker.RangePicker
              value={dateRange}
              onChange={(value) => {
                setDateRange(
                  value?.[0] && value[1]
                    ? [value[0], value[1]]
                    : undefined
                );
                setPage(1);
              }}
              style={{ width: "100%" }}
            />
          </div>

        </div>
        <div className="history-filter-actions">
          <Button icon={<ReloadOutlined />} onClick={clearFilters}>
            Đặt lại
          </Button>
        </div>

        {error && (
          <Alert
            type="error"
            showIcon
            message="Không thể tải lịch sử đo"
            description={error}
          />
        )}
        {loading ? (
          <div style={{ padding: 48, textAlign: "center" }}>
            <Spin tip="Đang tải lịch sử đo..." />
          </div>
        ) : data.length === 0 ? (
          <Empty description="Không có kết quả đo phù hợp." />
        ) : (
          <Table<BackendMeasurement>
            className="history-table"
            rowKey="id"
            columns={columns}
            dataSource={data}
            pagination={pagination}
            scroll={{ x: 1136 }}
          />
        )}
      </Card>
    </div>
  );
}

export default MeasurementHistory;
