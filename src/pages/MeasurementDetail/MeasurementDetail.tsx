
import { ArrowLeftOutlined } from "@ant-design/icons";
import { Alert, Card, Col, Empty, Row, Spin, Table, Tag } from "antd";
import type { ColumnsType } from "antd/es/table";
import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";

import PageHeader from "../../components/PageHeader";
import {
  getMeasurementById,
  type BackendMeasurement,
} from "../../services/measurementService";

type Result = "OK" | "NG" | "SKIPPED" | "INCOMPLETE";

function ResultTag({ result }: { result: Result }) {
  return (
    <Tag color={result === "OK" ? "green" : result === "NG" ? "red" : "default"}>
      {result}
    </Tag>
  );
}

function MeasurementDetail() {
  const navigate = useNavigate();
  const { measurementId } = useParams();

  const [measurement, setMeasurement] = useState<BackendMeasurement>();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    const id = Number(measurementId);

    if (!Number.isInteger(id) || id <= 0) {
      setError("Mã bản ghi không hợp lệ.");
      setLoading(false);
      return;
    }

    let active = true;

    void getMeasurementById(id)
      .then((value) => {
        if (active) setMeasurement(value);
      })
      .catch((reason: unknown) => {
        if (active) {
          setError(
            typeof reason === "string"
              ? reason
              : "Không thể tải chi tiết phép đo từ cơ sở dữ liệu."
          );
        }
      })
      .finally(() => {
        if (active) setLoading(false);
      });

    return () => {
      active = false;
    };
  }, [measurementId]);

  if (loading) {
    return (
      <div style={{ padding: 80, textAlign: "center" }}>
        <Spin tip="Đang tải chi tiết phép đo..." />
      </div>
    );
  }

  if (error) {
    return (
      <>
        <PageHeader
          eyebrow="Bản ghi phép đo"
          title="Không thể tải dữ liệu"
          description="Đã xảy ra lỗi khi truy xuất chi tiết phép đo."
          extra={
            <button
              className="ant-btn"
              onClick={() => navigate("/measurements")}
            >
              <ArrowLeftOutlined /> Quay lại lịch sử đo
            </button>
          }
        />

        <Alert type="error" showIcon message={error} />
      </>
    );
  }

  if (!measurement) {
    return <Empty description="Không tìm thấy phép đo." />;
  }

  const tipColumns: ColumnsType<BackendMeasurement["tipPositions"][number]> = [
    {
      title: "Vị trí",
      dataIndex: "positionNumber",
      render: (value: number) => `Tip ${value}`,
    },
    {
      title: "Khoảng cách cuối (mm)",
      dataIndex: "finalDistance",
      render: (value: string | null) => value ?? "—",
    },
    {
      title: "Số lần kiểm tra lại",
      dataIndex: "retryCount",
      render: (value: number | null) => value ?? "—",
    },
    {
      title: "Kết quả",
      dataIndex: "result",
      render: (value: Result) => <ResultTag result={value} />,
    },
  ];

  const overallColor =
    measurement.overallResult === "OK" ? "#1d9b6c" : "#d95757";

  return (
    <>
      <PageHeader
        eyebrow="Bản ghi phép đo"
        title={`#${measurement.id}`}
        description={`Ghi nhận lúc ${new Date(measurement.measuredAt).toLocaleString("vi-VN")}`}
        extra={
          <button
            className="ant-btn"
            onClick={() => navigate("/measurements")}
          >
            <ArrowLeftOutlined /> Quay lại lịch sử đo
          </button>
        }
      />

      <Row gutter={[20, 20]}>
        <Col xs={24} lg={9}>
          <Card className="section-card">
            <div className="card-heading">
              <div className="card-title">A. Thông tin chung</div>
              <ResultTag result={measurement.overallResult} />
            </div>

            <div className="detail-kv">
              <span className="detail-kv-label">Mã bản ghi</span>
              <span className="detail-kv-value">#{measurement.id}</span>
            </div>

            <div className="detail-kv">
              <span className="detail-kv-label">Mã sản phẩm</span>
              <span className="detail-kv-value">
                {measurement.productCode}
              </span>
            </div>

            <div className="detail-kv">
              <span className="detail-kv-label">Số thứ tự gậy</span>
              <span className="detail-kv-value">
                {measurement.clubNumber}
              </span>
            </div>

            <div className="detail-kv">
              <span className="detail-kv-label">Thời gian đo</span>
              <span className="detail-kv-value">
                {new Date(measurement.measuredAt).toLocaleString("vi-VN")}
              </span>
            </div>

            <div className="detail-kv">
              <span className="detail-kv-label">Weight</span>
              <span className="detail-kv-value">
                {measurement.weight}
              </span>
            </div>
          </Card>

          <Card className="section-card" style={{ marginTop: 20 }}>
            <div className="card-title" style={{ marginBottom: 16 }}>
              C. Trạm Butt
            </div>

            <div className="detail-kv">
              <span className="detail-kv-label">Khoảng cách cuối</span>
              <span className="detail-kv-value">
                {measurement.buttDistance} mm
              </span>
            </div>

            <div className="detail-kv">
              <span className="detail-kv-label">Kết quả Butt</span>
              <span className="detail-kv-value">
                <ResultTag result={measurement.buttResult} />
              </span>
            </div>
          </Card>
        </Col>

        <Col xs={24} lg={15}>
          <Card className="section-card">
            <div className="card-heading">
              <div>
                <div className="card-title">B. Trạm Tip</div>
                <div className="card-caption">Đầy đủ 4 vị trí đo</div>
              </div>

              <ResultTag result={measurement.tipResult} />
            </div>

            <Table
              rowKey="positionNumber"
              columns={tipColumns}
              dataSource={measurement.tipPositions}
              pagination={false}
            />
          </Card>

          <Card className="section-card" style={{ marginTop: 20 }}>
            <div className="card-title" style={{ marginBottom: 14 }}>
              D. Kết quả tổng thể
            </div>

            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 16,
                padding: "8px 0",
              }}
            >
              <div
                style={{
                  width: 48,
                  height: 48,
                  display: "grid",
                  placeItems: "center",
                  borderRadius: 14,
                  background:
                    measurement.overallResult === "OK"
                      ? "#e4f6ed"
                      : "#fff0ef",
                  color: overallColor,
                  fontSize: 24,
                }}
              >
                ●
              </div>

              <div>
                <div
                  style={{
                    fontFamily: "Space Grotesk",
                    fontSize: 20,
                    fontWeight: 700,
                    color: overallColor,
                  }}
                >
                  {measurement.overallResult === "OK"
                    ? "Overall OK"
                    : "Overall NG"}
                </div>

                <div className="card-caption">
                  {measurement.overallResult === "OK"
                    ? "Kết quả Tip và Butt đều đạt."
                    : "Có ít nhất một trạm không đạt."}
                </div>
              </div>
            </div>
          </Card>
        </Col>
      </Row>
    </>
  );
}

export default MeasurementDetail;
