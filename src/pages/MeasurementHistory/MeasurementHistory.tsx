
import { EyeOutlined, FilterOutlined, SearchOutlined } from "@ant-design/icons";
import { Alert, Button, Card, DatePicker, Empty, Input, Select, Spin, Table, Tag } from "antd";
import type { ColumnsType, TablePaginationConfig } from "antd/es/table";
import type { Dayjs } from "dayjs";
import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";

import PageHeader from "../../components/PageHeader";
import {
  getMeasurements,
  type BackendMeasurement,
  type MeasurementListRequest,
} from "../../services/measurementService";

type Result = "OK" | "NG";

function ResultTag({ result }: { result: Result }) {
  return <Tag color={result === "OK" ? "green" : "red"}>{result}</Tag>;
}

function MeasurementHistory() {
  const navigate = useNavigate();

  const [search, setSearch] = useState("");
  const [overallResult, setOverallResult] = useState<Result>();
  const [tipResult, setTipResult] = useState<Result>();
  const [buttResult, setButtResult] = useState<Result>();
  const [dateRange, setDateRange] = useState<[Dayjs, Dayjs]>();
  const [data, setData] = useState<BackendMeasurement[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const loadMeasurements = useCallback(async (request: MeasurementListRequest) => {
    setLoading(true);
    setError("");

    try {
      const response = await getMeasurements(request);
      setData(response.items);
      setTotal(response.total);
    } catch (reason: unknown) {
      setData([]);
      setTotal(0);
      setError(
        typeof reason === "string"
          ? reason
          : "Không thể tải lịch sử đo từ cơ sở dữ liệu."
      );
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void loadMeasurements({
        page,
        pageSize: 10,
        search: search.trim() || undefined,
        result: overallResult,
        tipResult,
        buttResult,
        measuredFrom: dateRange?.[0].startOf("day").toISOString(),
        measuredTo: dateRange?.[1].endOf("day").toISOString(),
      });
    }, 250);

    return () => window.clearTimeout(timer);
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
      title: "Số thứ tự gậy",
      dataIndex: "clubNumber",
    },
    {
      title: "Thời gian đo",
      dataIndex: "measuredAt",
      render: (value: string) => new Date(value).toLocaleString("vi-VN"),
    },
    {
      title: "Weight",
      dataIndex: "weight",
    },
    {
      title: "Kết quả Tip",
      dataIndex: "tipResult",
      render: (value: Result) => <ResultTag result={value} />,
    },
    {
      title: "Kết quả Butt",
      dataIndex: "buttResult",
      render: (value: Result) => <ResultTag result={value} />,
    },
    {
      title: "Kết quả tổng thể",
      dataIndex: "overallResult",
      render: (value: Result) => <ResultTag result={value} />,
    },
    {
      title: "Thao tác",
      key: "action",
      align: "right",
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
    <>
      <PageHeader
        eyebrow="Bản ghi"
        title="Lịch sử đo"
        description="Tra cứu và xem chi tiết các kết quả đo đã ghi nhận."
      />

      <Card className="section-card">
        <div className="filter-row" style={{ marginBottom: 22 }}>
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

          <Button
            icon={<FilterOutlined />}
            onClick={clearFilters}
          >
            Xóa bộ lọc
          </Button>
        </div>

        {error ? (
          <Alert
            type="error"
            showIcon
            message="Không thể tải lịch sử đo"
            description={error}
          />
        ) : loading ? (
          <div style={{ padding: 48, textAlign: "center" }}>
            <Spin tip="Đang tải lịch sử đo..." />
          </div>
        ) : data.length === 0 ? (
          <Empty description="Không có kết quả đo phù hợp." />
        ) : (
          <Table<BackendMeasurement>
            rowKey="id"
            columns={columns}
            dataSource={data}
            pagination={pagination}
            scroll={{ x: 1100 }}
          />
        )}
      </Card>
    </>
  );
}

export default MeasurementHistory;
