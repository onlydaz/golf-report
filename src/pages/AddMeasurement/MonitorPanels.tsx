import { Alert, Button, Card, Empty, Select, Spin, Table } from "antd";
import { useNavigate } from "react-router-dom";
import { useState } from "react";
import type { ReactNode } from "react";
import { CartesianGrid, Line, LineChart, ReferenceLine, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { ResultTag } from "../../components/measurement/MeasurementDetails";
import type { BackendMeasurement, MeasurementDetail, MeasurementHistoryItem, MeasurementStatistics } from "../../services/measurementService";
import { displayPointResult, formatDecimal, isBusinessPoint, isUnmeasuredPoint, stationPointNumbers } from "../../utils/as200Report";
import { formatReportTime } from "../../utils/reportTime";
import { measurementSettings } from "../../config/measurementSettings";
import type { MeasurementChartSettings } from "../../config/measurementSettings";
import { bendDomain, bendExcess } from "../../utils/bendChart";

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

const decimal = (value: number) => value.toLocaleString("vi-VN", { minimumFractionDigits: 2, maximumFractionDigits: 20 });
const compactNumber = (value: number) => value.toLocaleString("vi-VN", { maximumFractionDigits: 20 });
const configured = (value: number | null | undefined, unit: string) => value == null ? "Chưa cấu hình" : `${decimal(value)} ${unit}`;
function bendLimitText(settings?: MeasurementChartSettings | null) {
  return settings?.bendLowerMm != null || settings?.bendUpperMm != null
    ? settings.bendLowerMm == null ? `≤ ${decimal(settings.bendUpperMm!)} mm`
      : settings.bendUpperMm == null ? `≥ ${decimal(settings.bendLowerMm)} mm`
        : `${decimal(settings.bendLowerMm)} - ${decimal(settings.bendUpperMm)} mm`
    : "Chưa cấu hình";
}

export function BendSettingsSummary({ settings, station }: {
  settings: MeasurementChartSettings; station: "BUTT" | "TIP";
}) {
  return <div className="monitor-chart-settings">
    <span>Lực đè<b>{settings.loadG == null ? "Chưa cấu hình" : `${compactNumber(settings.loadG)} g`}</b></span>
    <span>Khoảng cách đẩy<b>{configured(settings.pushDistMm, "mm")}</b></span>
    <span>Giới hạn độ cong<b>{bendLimitText(settings)}</b></span>
    <span>Vị trí điểm đo<b className="monitor-position-values">{settings.positionsMm.some(v => v != null)
      ? <>{stationPointNumbers(station).map((_, i) => <span key={i}>
        {i > 0 && <span className="monitor-position-separator"> · </span>}
        {settings.positionsMm[i] == null ? "Chưa cấu hình" : compactNumber(settings.positionsMm[i]!)}
      </span>)} mm</> : "Chưa cấu hình"}</b></span>
  </div>;
}

export function BendDot({ cx, cy, value, color, settings }: {
  cx?: number; cy?: number; value?: number | null; color: string; settings?: MeasurementChartSettings | null;
}) {
  if (cx == null || cy == null || value == null || !Number.isFinite(value)) return <g />;
  const exceeded = (bendExcess(value, settings) ?? 0) > 0;
  return <circle cx={cx} cy={cy} r={exceeded ? 5 : 3}
    fill={exceeded ? "white" : color} stroke={exceeded ? "#d92d20" : color} strokeWidth={exceeded ? 2 : 1} />;
}

export function BendComparison({ records, productCode, loading, error }: {
  records: MeasurementHistoryItem[]; productCode?: string; loading: boolean; error: string;
}) {
  const [station, setStation] = useState<"BUTT" | "TIP">("BUTT");
  const settings = measurementSettings[station];
  const series = buildBendSeries(records, productCode, station);
  const limits = settings?.bendLowerMm != null || settings?.bendUpperMm != null;
  const limitText = bendLimitText(settings);
  const keys = ["p1", "p2", "p3", "p4"] as const;
  const excess = (value: number) => bendExcess(value, settings);
  const colors = ["#1769aa", "#ed8b47", "#1d9b6c", "#d4a82c"].slice(0, stationPointNumbers(station).length);
  const hasBend = series.some(row => [row.p1, row.p2, row.p3, row.p4].some(value => value != null));
  return <Card className="section-card monitor-bend" title="Độ cong thực tế · 30 gậy gần nhất"
    extra={<Select aria-label="Trạm biểu đồ độ cong" value={station} onChange={setStation}
      options={[{ value: "BUTT", label: "Trạm BUTT" }, { value: "TIP", label: "Trạm TIP" }]} />}>
    <BendSettingsSummary settings={settings} station={station} />
    <div className="monitor-chart-legend" aria-label="Chú giải điểm đo">{colors.map((color, i) => <span key={color}><i style={{ background: color }} />Điểm {i + 1}</span>)}
      {limits && <span><i className="monitor-limit-dot" />Vượt giới hạn cài đặt</span>}
    </div>
    {error && <Alert type="error" showIcon title={error} />}
    <Spin spinning={loading}>
      {hasBend ? <div className="monitor-bend-chart"><ResponsiveContainer width="100%" height="100%">
        <LineChart data={series} margin={{ top: 16, right: 18, bottom: 8, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" />
          <XAxis dataKey="id" tickFormatter={id => `#${series.find(row => row.id === id)?.clubNumber ?? "—"}`} />
          <YAxis width={65} domain={bendDomain(series.flatMap(row => keys.map(key => row[key])), settings)}
            label={{ value: "Độ cong (mm)", angle: -90, position: "insideLeft" }} />
          {settings?.bendLowerMm != null && <ReferenceLine y={settings.bendLowerMm} stroke="#d92d20" strokeDasharray="5 4" ifOverflow="extendDomain"
            label={{ value: `Dưới ${decimal(settings.bendLowerMm)}`, position: "insideBottomRight", fill: "#b42318", fontSize: 13 }} />}
          {settings?.bendUpperMm != null && <ReferenceLine y={settings.bendUpperMm} stroke="#d92d20" strokeDasharray="5 4" ifOverflow="extendDomain"
            label={{ value: `Trên ${decimal(settings.bendUpperMm)}`, position: "insideTopRight", fill: "#b42318", fontSize: 13 }} />}
          <Tooltip content={({ active, label }) => {
            const row = series.find(item => item.id === Number(label));
            if (!active || !row) return null;
            return <div className="monitor-bend-tooltip"><strong>{station} · Gậy #{row.clubNumber}</strong>
              <div>{formatReportTime(row.measuredAt)}</div>
              <div>Giới hạn cài đặt hiện tại: {limitText}</div>
              {colors.map((color, i) => {
                const value = row[keys[i]];
                const exceeded = value != null ? excess(value) : null;
                return <div key={keys[i]} style={{ color: exceeded != null && exceeded > 0 ? "#d92d20" : color }}>
                  {station} · P{i + 1} · Vị trí cài đặt: {configured(settings?.positionsMm[i], "mm")}
                  {` · Độ cong thực tế: ${value != null ? `${decimal(value)} mm` : "—"}`}
                  {value != null && exceeded != null && exceeded > 0
                    ? ` · Vượt ${settings?.bendLowerMm != null && value < settings.bendLowerMm ? "dưới" : "trên"} ${decimal(exceeded)} mm` : ""}
                </div>;
              })}
            </div>;
          }} />
          {colors.map((color, i) => {
            const dot = (props: { cx?: number; cy?: number; value?: number | null }) =>
              <BendDot key={`${i}-${props.cx}-${props.cy}`} {...props} color={color} settings={settings} />;
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
