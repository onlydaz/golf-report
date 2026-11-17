import { Alert, Button, Card, Empty, Select, Spin, Table } from "antd";
import { useNavigate } from "react-router-dom";
import { useState } from "react";
import type { ReactNode } from "react";
import { CartesianGrid, Line, LineChart, ReferenceLine, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { ResultTag } from "../../components/measurement/MeasurementDetails";
import type { BackendMeasurement, MeasurementDetail, MeasurementHistoryItem, MeasurementStatistics } from "../../services/measurementService";
import { displayPointResult, formatDecimal, isBusinessPoint, isUnmeasuredPoint, stationPointNumbers } from "../../utils/as200Report";
import { formatReportTime } from "../../utils/reportTime";

export function ProductionSummary({ productCode, statistics, loading, skuEditor }: {
  productCode?: string; statistics?: MeasurementStatistics; loading: boolean; skuEditor?: ReactNode;
}) {
  const count = (value: number | undefined) => loading && productCode ? <Spin size="small" /> : value?.toLocaleString("vi-VN") ?? "—";
  return <>
    <Card className="section-card monitor-sku">
      <div className="monitor-kicker">SKU đang chạy</div>
      {skuEditor ?? <strong className="monitor-sku-value">{productCode || "—"}</strong>}
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

export function decisionBend(row: MeasurementHistoryItem, station: "BUTT" | "TIP", pointNumber: number) {
  if (!isBusinessPoint(station, pointNumber)) return null;
  return row.decisionPoints.find(point => point.measurementId === row.id
    && point.station === station && point.pointNumber === pointNumber)?.bend ?? null;
}

export function buildBendSeries(records: MeasurementHistoryItem[], productCode: string | undefined, station: "BUTT" | "TIP") {
  if (!productCode) return [];
  return [...new Map(records.filter(row => row.productCode === productCode).map(row => [row.id, row])).values()]
    .sort((a, b) => Date.parse(b.measuredAt) - Date.parse(a.measuredAt) || b.id - a.id)
    .slice(0, 30).reverse().map(row => {
      const points = stationPointNumbers(station).map(point => {
        const value = decisionBend(row, station, point);
        return value != null && value.trim() !== "" && Number.isFinite(Number(value)) ? Number(value) : null;
      });
      return { id: row.id, sourceId: row.sourceId, clubNumber: row.clubNumber, measuredAt: row.measuredAt, recordSeq: row.recordSeq,
        p1: points[0] ?? null, p2: points[1] ?? null, p3: points[2] ?? null, p4: points[3] ?? null };
    });
}

// Display-only defaults for the Python DEMO fixture. Never inferred from measured values.
const simulatorDemoSettings = {
  BUTT: { load: 500, pushDist: 10, bendMin: 0, bendMax: 2, positions: [150, 300, 450, 600] },
  TIP: { load: 200, pushDist: 8, bendMin: 0, bendMax: 2, positions: [80] },
} as const;

export function BendComparison({ records, productCode, loading, error, simulatorSourceId }: {
  records: MeasurementHistoryItem[]; productCode?: string; loading: boolean; error: string; simulatorSourceId?: number;
}) {
  const [station, setStation] = useState<"BUTT" | "TIP">("BUTT");
  const series = buildBendSeries(records, productCode, station);
  // A dedicated new SKU avoids reinterpreting older TEST-AS200 fixtures with different samples.
  // Mixed-source history must not receive a shared DEMO limit, even with the same SKU.
  const demo = productCode === "DEMO-AS200" && simulatorSourceId != null && series.length > 0
    && series.every(row => row.sourceId === simulatorSourceId) ? simulatorDemoSettings[station] : undefined;
  const decimal = (value: number) => value.toLocaleString("vi-VN", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  const keys = ["p1", "p2", "p3", "p4"] as const;
  const excess = (value: number) => demo ? Math.max(demo.bendMin - value, value - demo.bendMax, 0) : null;
  const colors = ["#1769aa", "#ed8b47", "#1d9b6c", "#d4a82c"].slice(0, stationPointNumbers(station).length);
  const hasBend = series.some(row => [row.p1, row.p2, row.p3, row.p4].some(value => value != null));
  return <Card className="section-card monitor-bend" title="Độ cong thực tế · 30 gậy gần nhất"
    extra={<Select aria-label="Trạm biểu đồ độ cong" value={station} onChange={setStation}
      options={[{ value: "BUTT", label: "Trạm BUTT" }, { value: "TIP", label: "Trạm TIP" }]} />}>
    <div className="monitor-chart-settings">
      <span>Lực đè cài đặt<b>{demo ? demo.load : "—"}</b></span>
      <span>Khoảng cách đẩy<b>{demo ? decimal(demo.pushDist) : "—"}</b></span>
      <span>Giới hạn độ cong<b>{demo ? `${decimal(demo.bendMin)} – ${decimal(demo.bendMax)}` : "—"}</b></span>
      <span>Vị trí điểm đo<b>{demo ? demo.positions.join(" · ") : "—"}</b></span>
    </div>
    <div className="monitor-chart-legend" aria-label="Chú giải điểm đo">{colors.map((color, i) => <span key={color}><i style={{ background: color }} />Điểm {i + 1}</span>)}
      {demo && <span><i className="monitor-limit-dot" />Vượt giới hạn (NG)</span>}
    </div>
    {error && <Alert type="error" showIcon title={error} />}
    <Spin spinning={loading}>
      {hasBend ? <div className="monitor-bend-chart"><ResponsiveContainer width="100%" height="100%">
        <LineChart data={series} margin={{ top: 16, right: 18, bottom: 8, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" />
          <XAxis dataKey="id" tickFormatter={id => `#${series.find(row => row.id === id)?.clubNumber ?? "—"}`} />
          <YAxis width={65} label={{ value: "Độ cong (mm)", angle: -90, position: "insideLeft" }} />
          {demo && <>
            <ReferenceLine y={demo.bendMin} stroke="#d92d20" strokeDasharray="5 4" ifOverflow="extendDomain"
              label={{ value: `Dưới ${decimal(demo.bendMin)}`, position: "insideBottomRight", fill: "#b42318", fontSize: 13 }} />
            <ReferenceLine y={demo.bendMax} stroke="#d92d20" strokeDasharray="5 4" ifOverflow="extendDomain"
              label={{ value: `Trên ${decimal(demo.bendMax)}`, position: "insideTopRight", fill: "#b42318", fontSize: 13 }} />
          </>}
          <Tooltip content={({ active, label }) => {
            const row = series.find(item => item.id === Number(label));
            if (!active || !row) return null;
            return <div className="monitor-bend-tooltip"><strong>{station} · Gậy #{row.clubNumber}</strong>
              <div>{formatReportTime(row.measuredAt)}</div>
              <div>Giới hạn: {demo ? `${decimal(demo.bendMin)} – ${decimal(demo.bendMax)} mm (DEMO)` : "Chưa có dữ liệu cài đặt"}</div>
              {colors.map((color, i) => {
                const value = row[keys[i]];
                const exceeded = value != null ? excess(value) : null;
                return <div key={keys[i]} style={{ color: exceeded != null && exceeded > 0 ? "#d92d20" : color }}>
                  {station} · P{i + 1}: {value != null ? `${decimal(value)} mm` : "—"}
                  {demo && value != null && exceeded != null && exceeded > 0
                    ? ` · Vượt ${value < demo.bendMin ? "dưới" : "trên"} ${decimal(exceeded)} mm` : ""}
                </div>;
              })}
            </div>;
          }} />
          {colors.map((color, i) => {
            const dot = ({ cx, cy, value }: { cx?: number; cy?: number; value?: number | null }) => {
              if (cx == null || cy == null || value == null || !Number.isFinite(value)) return <g />;
              const ng = (excess(value) ?? 0) > 0;
              return <circle key={`${i}-${cx}-${cy}`} cx={cx} cy={cy} r={ng ? 5 : 3}
                fill={ng ? "#d92d20" : color} stroke={ng ? "white" : color} strokeWidth={ng ? 2 : 1} />;
            };
            return <Line key={color} type="linear" dataKey={keys[i]} name={`${station} · Điểm ${i + 1}`} stroke={color}
              connectNulls={false} dot={dot} activeDot={dot} isAnimationActive={false} />;
          })}
        </LineChart>
      </ResponsiveContainer></div> : <div className="monitor-chart-empty">
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={productCode ? `Chưa có dữ liệu độ cong đã lưu cho trạm ${station}` : "Chưa xác định SKU đang hiển thị"} />
      </div>}
    </Spin>
    <div className="monitor-chart-axis"><span>Gậy cũ hơn</span><span>Thứ tự gậy →</span><span>Gậy mới nhất</span></div>
  </Card>;
}

export function CurrentPointTable({ detail, loading }: { detail?: MeasurementDetail; loading: boolean }) {
  const rows = (["BUTT", "TIP"] as const).flatMap(type => detail?.stations.filter(station => station.stationType === type).flatMap(station =>
    station.points.filter(point => isBusinessPoint(type, point.pointNumber)).sort((a, b) => a.pointNumber - b.pointNumber).map(point => {
      const decision = point.attempts.find(attempt => attempt.attemptNumber === point.decideAttempt);
      return { ...point, result: displayPointResult(station, point), key: `${type}-${point.id}`, station: type, load: isUnmeasuredPoint(point) ? null : station.load,
        pushDist: decision?.pushDist, bend: decision?.bend };
    })
  ) ?? []);
  return <Table className="monitor-point-table" size="small" rowKey="key" dataSource={rows} loading={loading} pagination={false} tableLayout="fixed"
    locale={{ emptyText: "Chưa có chi tiết đo đã lưu của gậy này." }} columns={[
      { title: "Trạm / Điểm", render: (_, row) => `${row.station} · P${row.pointNumber}` },
      { title: "Vị trí (mm)", render: (_, row) => formatDecimal(isUnmeasuredPoint(row) ? null : row.position), width: "13%" },
      { title: "Lực đo (g)", dataIndex: "load", render: formatDecimal, width: "13%" },
      { title: "Khoảng cách đẩy (mm)", dataIndex: "pushDist", render: formatDecimal, width: "17%" },
      { title: "Độ cong (mm)", dataIndex: "bend", render: formatDecimal, width: "14%" },
      { title: <span className="monitor-attempt-heading">Số<br />lần đo</span>, dataIndex: "attemptCount", align: "center", render: (_, row) => isUnmeasuredPoint(row) ? "—" : row.attemptCount, width: 54 },
      { title: "Kết quả", dataIndex: "result", render: result => <ResultTag result={result} qcLabels />, width: "13%" },
    ]} />;
}

export function RecentMeasurements({ records, loading, error, onRefresh, page, total, onPageChange }: {
  records: MeasurementHistoryItem[]; loading: boolean; error: string; onRefresh: () => void;
  page: number; total: number; onPageChange: (page: number) => void;
}) {
  const navigate = useNavigate();
  const bends = (row: MeasurementHistoryItem, type: "BUTT" | "TIP") =>
    stationPointNumbers(type).map(point => formatDecimal(decisionBend(row, type, point))).join(" / ");
  return <Card className="section-card monitor-recent" title="Các gậy gần nhất"
    extra={<div className="monitor-recent-actions"><Button type="text" size="small" loading={loading} onClick={onRefresh}>Làm mới</Button><Button type="link" size="small" onClick={() => navigate("/measurements")}>Xem toàn bộ lịch sử →</Button></div>}>
    {error && <Alert type="error" showIcon title={error} className="add-measurement-alert" />}
    <Table<MeasurementHistoryItem> rowKey="id" size="small" loading={loading} dataSource={records}
      pagination={{ current: page, total, pageSize: 6, onChange: onPageChange, hideOnSinglePage: true, showSizeChanger: false }} scroll={{ x: 950 }}
      locale={{ emptyText: "Chưa có kết quả đo đã lưu." }}
      onRow={row => ({ onClick: () => navigate(`/measurements/${row.id}`), style: { cursor: "pointer" } })}
      columns={[
        { title: "Thời gian", dataIndex: "measuredAt", render: formatReportTime, width: 165 },
        { title: "Số gậy", dataIndex: "clubNumber", render: value => `#${value}`, width: 80 },
        { title: "BUTT P1 / P2 / P3 / P4 (mm)", render: (_, row) => bends(row, "BUTT"), width: 260 },
        { title: <span title="Độ cong TIP P1 tại lượt đo quyết định">TIP (mm)</span>, render: (_, row) => bends(row, "TIP"), width: 112 },
        ...([ ["BUTT", "buttResult"], ["TIP", "tipResult"], ["Overall", "overallResult"] ] as const).map(([title, dataIndex]) => ({ title, dataIndex, render: (value: BackendMeasurement["buttResult"]) => <ResultTag result={value} />, width: 85 })),
      ]} />
  </Card>;
}
