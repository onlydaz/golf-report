import { ArrowLeftOutlined } from "@ant-design/icons";
import { Alert, Button, Empty, Spin } from "antd";
import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";

import PageHeader from "../../components/PageHeader";

import {
  getMeasurementById,
  type MeasurementDetail as MeasurementData,
} from "../../services/measurementService";

import MeasurementDetails from "../../components/measurement/MeasurementDetails";
import "../AddMeasurement/AddMeasurement.css";
import "./MeasurementDetail.css";

function MeasurementDetail() {
  const navigate = useNavigate();
  const { measurementId } = useParams();

  const [data, setData] = useState<MeasurementData>();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;

    setData(undefined);
    setLoading(true);
    setError("");

    const id = Number(measurementId);

    if (!Number.isSafeInteger(id) || id <= 0) {
      setError("Mã bản ghi không hợp lệ.");
      setLoading(false);
      return;
    }

    void getMeasurementById(id)
      .then(value => {
        if (active) setData(value);
      })
      .catch(reason => {
        if (active) {
          setError(
            typeof reason === "string"
              ? reason
              : "Không tải được phép đo."
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

  return (
    <div className="measurement-detail-page add-measurement-details-card">
      <PageHeader
        eyebrow="AS200"
        title="Chi tiết phép đo"
        description="Thông tin chi tiết và kết quả đo tại các trạm Tip và Butt."
        extra={
          <Button
            icon={<ArrowLeftOutlined />}
            onClick={() => navigate("/measurements")}
          >
            Quay lại lịch sử đo
          </Button>
        }
      />

      {loading ? (
        <Spin />
      ) : error ? (
        <Alert type="error" message={error} />
      ) : !data ? (
        <Empty description="Chưa có dữ liệu" />
      ) : (
        <MeasurementDetails data={data} />
      )}
    </div>
  );
}

export default MeasurementDetail;
