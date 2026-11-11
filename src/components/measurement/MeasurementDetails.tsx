import { formatReportTime } from "../../utils/reportTime";
import { Card, Descriptions, Empty, Table, Tag } from "antd";

import type {
  MeasurementDetail,
  MeasurementPoint,
  MeasurementResult,
} from "../../services/measurementService";

import { formatDecimal } from "../../utils/as200Report";

export function ResultTag({
  result,
  qcLabels = false,
}: {
  result?: MeasurementResult | null;
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
                children: data.clubNumber,
              },
              {
                key: "seq",
                label: "Mã lượt đo (RecordSeq)",
                children: data.recordSeq,
              },
              {
                key: "total",
                label: "Tổng số lượt đo",
                children: data.totalCount ?? "—",
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
                <Descriptions
                  className="station-summary"
                  column={{ xs: 1, sm: 2, lg: 4 }}
                  items={[
                    {
                      key: "load",
                      label: "Khối lượng đè (g)",
                      children: formatDecimal(station.load),
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
                      children: station.pointCount,
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
                  dataSource={[...station.points].sort(
                    (a, b) => a.pointNumber - b.pointNumber
                  )}
                  scroll={{ x: 640 }}
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
                      render: formatDecimal,
                      align: "right",
                      width: 160,
                    },
                    {
                      title: "Kết quả",
                      dataIndex: "result",
                      render: (value: MeasurementResult) => (
                        <ResultTag
                          result={value}
                          qcLabels={qcLabels}
                        />
                      ),
                      align: "center",
                      width: 110,
                    },
                    {
                      title: "Số lượt đo",
                      dataIndex: "attemptCount",
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
                  ]}
                  expandable={{
                    rowExpandable: point => point.attempts.length > 0,
                    expandedRowRender: point => (
                      <Table
                        className="measurement-attempt-table"
                        size="small"
                        rowKey="id"
                        pagination={false}
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
                            title: "PushDist (mm)",
                            dataIndex: "pushDist",
                            render: formatDecimal,
                            align: "right",
                            width: 150,
                          },
                          {
                            title: "Bend (mm)",
                            dataIndex: "bend",
                            render: formatDecimal,
                            align: "right",
                            width: 150,
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
