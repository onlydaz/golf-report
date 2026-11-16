import { Alert, Button, Card, Empty, Spin, Table } from "antd";
import { useNavigate } from "react-router-dom";
import { ResultTag } from "../../components/measurement/MeasurementDetails";
import type { BackendMeasurement, MeasurementDetail, MeasurementStatistics } from "../../services/measurementService";
import { formatDecimal, isUnmeasuredPoint } from "../../utils/as200Report";
import { formatReportTime } from "../../utils/reportTime";

export function ProductionSummary({ productCode, statistics, loading }: {
  productCode?: string; statistics?: MeasurementStatistics; loading: boolean;
}) {
  const count = (value: number | undefined) => loading && productCode ? <Spin size="small" /> : value?.toLocaleString("vi-VN") ?? "—";
  return <>
    <Card className="section-card monitor-sku">
      <div className="monitor-kicker">SKU đang chạy</div>
      <strong className="monitor-sku-value">{productCode || "—"}</strong>
    </Card>
    <Card className="section-card monitor-count monitor-count-ok"><div className="monitor-kicker">OK</div>
      <strong>{count(statistics?.totalOk)}</strong><small>{statistics ? `${statistics.passRate.toLocaleString("vi-VN", { maximumFractionDigits: 1 })}% tổng số gậy` : "Chưa có thống kê"}</small>
    </Card>
    <Card className="section-card monitor-count monitor-count-ng"><div className="monitor-kicker">NG</div>
      <strong>{count(statistics?.totalNg)}</strong><small>{statistics?.totalMeasurements ? `${(statistics.totalNg * 100 / statistics.totalMeasurements).toLocaleString("vi-VN", { maximumFractionDigits: 1 })}% tổng số gậy` : "Theo mã sản phẩm"}</small>
    </Card>
    <Card className="section-card monitor-count monitor-count-total"><div className="monitor-kicker">Tổng số gậy</div>
      <strong>{count(statistics?.totalMeasurements)}</strong><small>Tổng số gậy theo SKU</small>
    </Card>
    <Card className="section-card monitor-stations"><div className="monitor-kicker">Theo trạm</div>
      <div className="monitor-station-columns">{(["BUTT", "TIP"] as const).map(type => {
        const station = statistics?.stationResults.find(item => item.station.toUpperCase() === type);
        return <div key={type}><b>{type}</b><div className="monitor-station-counts"><span>OK<strong>{count(station?.ok)}</strong></span><span>NG<strong>{count(station?.ng)}</strong></span></div></div>;
      })}</div>
    </Card>
  </>;
}

export function BendComparison() {
  return <Card className="section-card monitor-bend" title="Độ cong so với cài đặt · Gậy gần nhất">
    <div className="monitor-chart-settings"><span>Load cài đặt <b>—</b></span><span>Giới hạn Bend <b>—</b></span><span>Vị trí cài đặt <b>—</b></span></div>
    <div className="monitor-chart-legend" aria-label="Chú giải điểm đo">{["#1769aa", "#ed8b47", "#1d9b6c", "#d4a82c"].map((color, i) => <span key={color}><i style={{ background: color }} />Điểm {i + 1}</span>)}</div>
    <div className="monitor-chart-empty">
      <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="Chưa có dữ liệu để đối chiếu độ cong với cài đặt" />
      <p className="monitor-caption">Chưa có setpoint theo SKU và chuỗi Bend của 30 gậy trong dữ liệu đang tải.</p>
    </div>
    <div className="monitor-chart-axis"><span>Gậy cũ hơn</span><span>Thứ tự gậy →</span><span>Gậy mới nhất</span></div>
  </Card>;
}

export function CurrentPointTable({ detail, loading }: { detail?: MeasurementDetail; loading: boolean }) {
  const rows = (["BUTT", "TIP"] as const).flatMap(type => detail?.stations.filter(station => station.stationType === type).flatMap(station =>
    [...station.points].sort((a, b) => a.pointNumber - b.pointNumber).map(point => {
      const decision = point.attempts.find(attempt => attempt.attemptNumber === point.decideAttempt);
      return { ...point, key: `${type}-${point.id}`, station: type, load: isUnmeasuredPoint(point) ? null : station.load,
        pushDist: decision?.pushDist, bend: decision?.bend };
    })
  ) ?? []);
  return <Table className="monitor-point-table" size="small" rowKey="key" dataSource={rows} loading={loading} pagination={false} scroll={{ x: 650 }}
    locale={{ emptyText: "Chưa có chi tiết đo đã lưu của gậy này." }} columns={[
      { title: "Trạm / Điểm", render: (_, row) => `${row.station} · P${row.pointNumber}`, width: 100 },
      { title: "Vị trí (mm)", render: (_, row) => formatDecimal(isUnmeasuredPoint(row) ? null : row.position), width: 85 },
      { title: "Lực đo (g)", dataIndex: "load", render: formatDecimal, width: 80 },
      { title: "Khoảng cách đẩy (mm)", dataIndex: "pushDist", render: formatDecimal, width: 100 },
      { title: "Độ cong (mm)", dataIndex: "bend", render: formatDecimal, width: 90 },
      { title: "Số lần đo", dataIndex: "attemptCount", render: (_, row) => isUnmeasuredPoint(row) ? "—" : row.attemptCount, width: 80 },
      { title: "Kết quả", dataIndex: "result", render: (result, row) => isUnmeasuredPoint(row) ? "—" : <ResultTag result={result} qcLabels />, width: 85 },
    ]} />;
}

export function RecentMeasurements({ records, detail, loading, error, onRefresh }: {
  records: BackendMeasurement[]; detail?: MeasurementDetail; loading: boolean; error: string; onRefresh: () => void;
}) {
  const navigate = useNavigate();
  const bends = (row: BackendMeasurement, type: "BUTT" | "TIP") => {
    if (detail?.id !== row.id) return "—";
    const station = detail.stations.find(item => item.stationType === type);
    return station ? [...station.points].sort((a, b) => a.pointNumber - b.pointNumber).map(point =>
      formatDecimal(point.attempts.find(attempt => attempt.attemptNumber === point.decideAttempt)?.bend)).join(" / ") : "—";
  };
  return <Card className="section-card monitor-recent" title="Các gậy gần nhất"
    extra={<div className="monitor-recent-actions"><Button type="text" size="small" loading={loading} onClick={onRefresh}>Làm mới</Button><Button type="link" size="small" onClick={() => navigate("/measurements")}>Xem toàn bộ lịch sử →</Button></div>}>
    {error && <Alert type="error" showIcon title={error} className="add-measurement-alert" />}
    <Table<BackendMeasurement> rowKey="id" size="small" loading={loading} dataSource={records}
      pagination={{ pageSize: 6, hideOnSinglePage: true, showSizeChanger: false }} scroll={{ x: 950 }}
      locale={{ emptyText: "Chưa có kết quả đo đã lưu." }}
      onRow={row => ({ onClick: () => navigate(`/measurements/${row.id}`), style: { cursor: "pointer" } })}
      columns={[
        { title: "Thời gian", dataIndex: "measuredAt", render: formatReportTime, width: 165 },
        { title: "Số gậy", dataIndex: "clubNumber", render: value => `#${value}`, width: 80 },
        { title: "BUTT P1 / P2 / P3 / P4 (mm)", render: (_, row) => bends(row, "BUTT"), width: 260 },
        { title: <span title="Bend các điểm TIP theo thứ tự điểm, không tự gộp thành một giá trị">TIP (mm)</span>, render: (_, row) => bends(row, "TIP"), width: 190 },
        ...([ ["BUTT", "buttResult"], ["TIP", "tipResult"], ["Overall", "overallResult"] ] as const).map(([title, dataIndex]) => ({ title, dataIndex, render: (value: BackendMeasurement["buttResult"]) => <ResultTag result={value} />, width: 85 })),
      ]} />
  </Card>;
}
