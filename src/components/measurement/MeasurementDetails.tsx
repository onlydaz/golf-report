import { formatReportTime } from "../../utils/reportTime";
import { Alert, Card, Descriptions, Empty, Table, Tag } from "antd";

import type {
  MeasurementDetail,
  MeasurementPoint,
  MeasurementResult,
} from "../../services/measurementService";

import { displayPointResult, formatClubNumber, formatDecimal, isBusinessPoint, isUnmeasuredPoint } from "../../utils/as200Report";
import { LoadValue, PositionValue } from "./MeasurementValue";

export function ResultTag({
  result,
  qcLabels = false,
}: {
  result?: MeasurementResult | "SKIPPED" | null;
  qcLabels?: boolean;
}) {
  return (
    <Tag color={result === "OK" ? "green" : result === "NG" ? "red" : "default"}>
      {String(result) === "SKIPPED" ? (qcLabels ? "BỎ QUA" : "Bỏ qua")
        : result === "UNMEASURED"
        ? (qcLabels ? "CHƯA ĐO" : "Chưa đo")
        : result ?? "—"}
    </Tag>
  );
}

export default function MeasurementDetails({
  data,
  qcLabels = false,
  showGeneralInformation = true,
}: {
  data: MeasurementDetail;
  qcLabels?: boolean;
  showGeneralInformation?: boolean;
}) {
  return (
    <div className="measurement-details">
      {data.qualityError && <Alert type="info" showIcon
        title={data.overallResult === "UNMEASURED" ? "Chưa xác định chất lượng" : "Bản ghi có thông tin cần kiểm tra"}
        description={`${data.overallResult === "UNMEASURED" ? "Bản ghi đã lưu, không tính vào KPI sản xuất." : "Kết quả và KPI giữ theo Overall do PLC cung cấp."} ${data.qualityError}`} />}
      {showGeneralInformation && (
        <Card className="section-card">
          <Descriptions
            column={{ xs: 1, sm: 2, lg: 3 }}
            items={[
              {
                key: "id",
                label: "Mã phép đo",
                children: data.id,
              },
              {
                key: "product",
                label: "Mã sản phẩm",
                children: data.productCode || "—",
              },
              {
                key: "club",
                label: "Số gậy",
                children: formatClubNumber(data.clubNumber),
              },
              {
                key: "time",
                label: "Thời gian nhận",
                children: formatReportTime(data.measuredAt),
              },
              {
                key: "result",
                label: "Kết quả tổng thể",
                children: (
                  <ResultTag
                    result={data.overallResult}
                    qcLabels={qcLabels}
                  />
                ),
              },
            ]}
          />
        </Card>
      )}

      {(["TIP", "BUTT"] as const).map(type => {
        const station = data.stations.find(
          item => item.stationType === type
        );

        return (
          <Card
            className="section-card"
            title={`Trạm ${type}`}
            key={type}
          >
            {!station ? (
              <Empty description="Chưa có dữ liệu trạm" />
            ) : (
              <>
                {type === "TIP" && station.points.some(point => !isBusinessPoint(type, point.pointNumber) && !isUnmeasuredPoint(point)) &&
                  <Alert type="warning" showIcon title="Dữ liệu TIP cũ có điểm ngoài P1. Giữ nguyên dữ liệu trong database để kỹ thuật đối chiếu; các điểm này không tính vào nghiệp vụ TIP hiện tại." />}
                <Descriptions
                  className="station-summary"
                  column={{ xs: 1, sm: 2, lg: 4 }}
                  items={[
                    {
                      key: "load",
                      label: "Lực đè (kg)",
                      children: <LoadValue value={station.load} station={type} />,
                    },
                    {
                      key: "result",
                      label: "Kết quả",
                      children: (
                        <ResultTag
                          result={station.result}
                          qcLabels={qcLabels}
                        />
                      ),
                    },
                    {
                      key: "count",
                      label: "Số điểm đo",
                      children: station.pointCount ?? "—",
                    },
                    {
                      key: "ng",
                      label: "Điểm NG",
                      children: station.ngPoint ?? "—",
                    },
                  ]}
                />

                <Table<MeasurementPoint>
                  className="measurement-point-table"
                  size="small"
                  rowKey="id"
                  pagination={false}
                  dataSource={station.points.filter(point => isBusinessPoint(type, point.pointNumber)).sort(
                    (a, b) => a.pointNumber - b.pointNumber
                  )}
                  tableLayout="fixed"
                  scroll={{ x: 1040 }}
                  columns={[
                    {
                      title: "Điểm",
                      dataIndex: "pointNumber",
                      align: "center",
                      width: 76,
                    },
                    {
                      title: "Vị trí đo (mm)",
                      dataIndex: "position",
                      render: (value: string | null, point) => <PositionValue value={isUnmeasuredPoint(point) ? null : value}
                        station={type} pointNumber={point.pointNumber} />,
                      align: "right",
                      width: 160,
                    },
                    {
                      title: "Kết quả",
                      dataIndex: "result",
                      render: (_: MeasurementResult, point) => (
                        <ResultTag
                          result={displayPointResult(station, point)}
                          qcLabels={qcLabels}
                        />
                      ),
                      align: "center",
                      width: 110,
                    },
                    {
                      title: "Số lượt đo",
                      dataIndex: "attemptCount",
                      render: (value: number, point) => isUnmeasuredPoint(point) ? "—" : value,
                      align: "center",
                      width: 115,
                    },
                    {
                      title: "Lượt quyết định",
                      dataIndex: "decideAttempt",
                      render: (value: number | null) => value ?? "—",
                      align: "center",
                      width: 145,
                    },
                    {
                      title: "Số lần đo lại",
                      render: (_, point) => point.attemptCount > 0 ? point.attemptCount - 1 : "—",
                      align: "center",
                      width: 120,
                    },
                    {
                      title: <>Khoảng cách đẩy<br />quyết định (mm)</>,
                      render: (_, point) => formatDecimal(point.attempts.find(
                        attempt => attempt.attemptNumber === point.decideAttempt
                      )?.pushDist),
                      align: "center",
                      width: 170,
                    },
                    {
                      title: <>Độ cong quyết<br />định (mm)</>,
                      render: (_, point) => formatDecimal(point.attempts.find(
                        attempt => attempt.attemptNumber === point.decideAttempt
                      )?.bend),
                      align: "center",
                      width: 150,
                    },
                  ]}
                  expandable={{
                    rowExpandable: point => point.attempts.length > 0,
                    expandedRowRender: point => (
                      <Table
                        className="measurement-attempt-table"
                        size="small"
                        rowKey="id"
                        pagination={false}
                        tableLayout="fixed"
                        scroll={{ x: 580 }}
                        dataSource={[...point.attempts].sort(
                          (a, b) => a.attemptNumber - b.attemptNumber
                        )}
                        columns={[
                          {
                            title: "Lượt đo",
                            dataIndex: "attemptNumber",
                            align: "center",
                            width: 100,
                          },
                          {
                            title: "Khoảng cách đẩy (mm)",
                            dataIndex: "pushDist",
                            render: formatDecimal,
                            align: "right",
                            width: 270,
                          },
                          {
                            title: "Độ cong (mm)",
                            dataIndex: "bend",
                            render: formatDecimal,
                            align: "right",
                            width: 210,
                          },
                        ]}
                      />
                    ),
                  }}
                />
              </>
            )}
          </Card>
        );
      })}
    </div>
  );
}
