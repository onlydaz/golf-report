import { FileExcelOutlined, FileTextOutlined, FilterOutlined, ReloadOutlined } from "@ant-design/icons";
import { Alert, Button, Card, DatePicker, Empty, Input, Select, Space, Spin, Table, message } from "antd";
import type { ColumnsType } from "antd/es/table";
import type { Dayjs } from "dayjs";
import dayjs from "dayjs";
import { save } from "@tauri-apps/plugin-dialog";
import { writeFile } from "@tauri-apps/plugin-fs";
import { useEffect, useRef, useState } from "react";
import {
  Bar, BarChart, CartesianGrid, Legend, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis,
} from "recharts";
import { useNavigate } from "react-router-dom";
import PageHeader from "../../components/PageHeader";
import { ResultTag } from "../../components/measurement/MeasurementDetails";
import { usePlcMonitor } from "../../hooks/usePlcMonitor";
import {
  getMeasurementStatistics, getMeasurementQualityStatistics, getMeasurementReport, getMeasurements, getMeasurementSkus,
  type BackendMeasurement, type MeasurementListRequest, type MeasurementStatistics,
  type MeasurementQualityStatistics, type PointQualityStatistics, type PointSummaryStatistics,
} from "../../services/measurementService";
import { createCsvBytes, createExcelBytes } from "../../utils/as200Export";
import { buildReportTables, formatDecimal } from "../../utils/as200Report";
import { formatReportTime, reportDateBoundary, vietnamNow } from "../../utils/reportTime";
import "./Reports.css";

type ResultFilter = "OK" | "NG" | "UNMEASURED" | undefined;
type Period = "today" | "week" | "custom";
const defaultRange = (): [Dayjs, Dayjs] => [vietnamNow().subtract(6, "day"), vietnamNow()];
const numeric = (value: string | null) => value == null || !Number.isFinite(Number(value))
  ? "—" : Number(value).toLocaleString("vi-VN", { maximumSignificantDigits: 9 });
const pointKey = (point: PointQualityStatistics) => JSON.stringify([point.productCode, point.station, point.pointNumber, point.position]);
const pointLabel = (point: PointQualityStatistics) => `${point.productCode} · ${point.station} P${point.pointNumber} · ${formatDecimal(point.position)} mm`;

function Reports() {
  const navigate = useNavigate();
  const { databaseConnected } = usePlcMonitor();
  const [messageApi, contextHolder] = message.useMessage();
  const [period, setPeriod] = useState<Period>("week");
  const [range, setRange] = useState<[Dayjs, Dayjs]>(defaultRange);
  const [product, setProduct] = useState("");
  const [skuSearch, setSkuSearch] = useState({ text: "", page: 1 });
  const [skuOptions, setSkuOptions] = useState<string[]>([]);
  const [skuLoading, setSkuLoading] = useState(false);
  const [skuHasMore, setSkuHasMore] = useState(false);
  const [skuError, setSkuError] = useState("");
  const skuRequestPending = useRef(false);
  const [club, setClub] = useState("");
  const [tip, setTip] = useState<ResultFilter>();
  const [butt, setButt] = useState<ResultFilter>();
  const [overall, setOverall] = useState<ResultFilter>();
  const [query, setQuery] = useState<{ filters: MeasurementListRequest; page: number }>(() => {
    const dates = defaultRange();
    return { page: 1, filters: { measuredFrom: reportDateBoundary(dates[0]), measuredTo: reportDateBoundary(dates[1], true) } };
  });
  const [stats, setStats] = useState<MeasurementStatistics>();
  const [quality, setQuality] = useState<MeasurementQualityStatistics>();
  const [records, setRecords] = useState<BackendMeasurement[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [listLoading, setListLoading] = useState(true);
  const [error, setError] = useState("");
  const [listError, setListError] = useState("");
  const [exporting, setExporting] = useState(false);
  const [selectedPoint, setSelectedPoint] = useState<string>();

  useEffect(() => {
    let active = true;
    skuRequestPending.current = databaseConnected;
    setSkuLoading(databaseConnected); setSkuError("");
    if (!databaseConnected) { setSkuOptions([]); setSkuHasMore(false); return; }
    const timer = setTimeout(() => {
      void getMeasurementSkus(skuSearch.text, skuSearch.page).then(response => {
        if (!active) return;
        setSkuOptions(previous => skuSearch.page === 1 ? response.items : [...new Set([...previous, ...response.items])]);
        setSkuHasMore(response.hasMore);
      }).catch(() => { if (active) { setSkuError("Không tải được danh sách SKU."); setSkuHasMore(false); } })
        .finally(() => { if (active) { setSkuLoading(false); skuRequestPending.current = false; } });
    }, 250);
    return () => { active = false; clearTimeout(timer); };
  }, [skuSearch, databaseConnected]);

  useEffect(() => {
    let active = true;
    setStats(undefined); setQuality(undefined); setSelectedPoint(undefined); setError("");
    setLoading(databaseConnected);
    if (!databaseConnected) return;
    void Promise.all([getMeasurementStatistics(query.filters), getMeasurementQualityStatistics(query.filters)])
      .then(([summary, points]) => {
        if (!active) return;
        setStats(summary); setQuality(points);
        const first = points.points.find(point => point.bendCount > 0);
        setSelectedPoint(first ? pointKey(first) : undefined);
      }).catch(() => { if (active) setError("Không thể tải báo cáo chất lượng. Vui lòng tạo báo cáo lại."); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [query.filters, databaseConnected]);

  useEffect(() => {
    let active = true;
    setRecords([]); setTotal(0); setListError(""); setListLoading(databaseConnected);
    if (!databaseConnected) return;
    void getMeasurements({ ...query.filters, page: query.page, pageSize: 10 }).then(response => {
      if (!active) return;
      const lastPage = Math.max(1, Math.ceil(response.total / 10));
      if (query.page > lastPage) { setQuery(current => ({ ...current, page: lastPage })); return; }
      setRecords(response.items); setTotal(response.total);
    }).catch(() => { if (active) setListError("Không thể tải danh sách phép đo."); })
      .finally(() => { if (active) setListLoading(false); });
    return () => { active = false; };
  }, [query, databaseConnected]);

  const apply = () => {
    const number = club.trim() ? Number(club) : undefined;
    if (number !== undefined && (!Number.isInteger(number) || number < -2147483648 || number > 2147483647)) {
      void messageApi.error("Số gậy phải là số nguyên signed 32-bit theo AS200."); return;
    }
    const dates: [Dayjs, Dayjs] = period === "today" ? [vietnamNow(), vietnamNow()]
      : period === "week" ? defaultRange() : range;
    if (dates[0].isAfter(dates[1], "day")) { void messageApi.error("Ngày bắt đầu phải trước hoặc bằng ngày kết thúc."); return; }
    setRange(dates);
    setQuery({ page: 1, filters: {
      measuredFrom: reportDateBoundary(dates[0]), measuredTo: reportDateBoundary(dates[1], true),
      productCode: product || undefined, clubNumber: number, tipResult: tip, buttResult: butt, result: overall,
    } });
  };
  const reset = () => {
    const dates = defaultRange();
    setPeriod("week"); setRange(dates); setProduct(""); setClub(""); setTip(undefined); setButt(undefined); setOverall(undefined);
    setSkuOptions([]); setSkuSearch({ text: "", page: 1 });
    setQuery({ page: 1, filters: { measuredFrom: reportDateBoundary(dates[0]), measuredTo: reportDateBoundary(dates[1], true) } });
  };

  const exportReport = async (format: "csv" | "xlsx") => {
    if (exporting) return;
    setExporting(true);
    try {
      // Explicit export is the only path loading full nested details, without pagination.
      const all = await getMeasurementReport(query.filters);
      if (!all.length) { void messageApi.info("Không có dữ liệu phù hợp để xuất."); return; }
      const path = await save({ defaultPath: `bao-cao-do-gay-${vietnamNow().format("YYYYMMDD-HHmmss")}.${format}`,
        filters: [{ name: format === "csv" ? "CSV" : "Excel", extensions: [format] }] });
      if (!path) return;
      const rows = buildReportTables(all).measurements;
      const bytes = format === "csv" ? createCsvBytes(rows, all) : await createExcelBytes(rows, all);
      await writeFile(path, bytes);
      void messageApi.success(`Đã xuất ${all.length} phép đo và chi tiết ra ${format === "csv" ? "CSV" : "Excel"}.`);
    } catch { void messageApi.error("Xuất báo cáo thất bại. Kiểm tra kết nối dữ liệu và quyền ghi file."); }
    finally { setExporting(false); }
  };

  const points = quality?.points ?? [];
  const chosen = points.find(point => pointKey(point) === selectedPoint);
  const topNg = [...points].filter(point => point.ng > 0).sort((a, b) => b.ng - a.ng).slice(0, 12)
    .map(point => ({ ...point, label: pointLabel(point), rate: 100 * point.ng / point.measured }));
  const mostNg = topNg[0];
  const daily = stats?.daily.map(item => ({ ...item, rate: item.total ? 100 * item.ok / item.total : null })) ?? [];
  const spanDays = query.filters.measuredFrom && query.filters.measuredTo
    ? dayjs(query.filters.measuredTo).diff(dayjs(query.filters.measuredFrom), "day") : daily.length;
  const timeUnit = spanDays > 180 ? "month" : spanDays > 60 ? "week" : "day";
  // Only daily aggregates come to the UI; large date ranges are combined for legibility.
  const grouped = new Map<string, { date: string; ok: number; ng: number }>();
  for (const item of daily) {
    const key = dayjs(item.date).startOf(timeUnit).format("YYYY-MM-DD");
    const entry = grouped.get(key) ?? { date: key, ok: 0, ng: 0 };
    entry.ok += item.ok; entry.ng += item.ng; grouped.set(key, entry);
  }
  const histogram = chosen?.histogram.map((bin, index) => ({ ...bin,
    label: bin.lower === bin.upper ? numeric(bin.lower) : `${numeric(bin.lower)}–${numeric(bin.upper)}`,
    bounds: `[${formatDecimal(bin.lower)}; ${formatDecimal(bin.upper)}${index === chosen.histogram.length - 1 ? "]" : ")"} mm`,
  })) ?? [];
  const cards = [
    ["Tổng số gậy", stats?.totalMeasurements, "blue"], ["Tổng OK", stats?.totalOk, "green"],
    ["Tổng NG", stats?.totalNg, "red"], ["Tỷ lệ OK", stats ? `${stats.passRate.toLocaleString("vi-VN", { maximumFractionDigits: 1 })}%` : undefined, "green"],
    ["NG Butt", stats?.buttNg, "red"], ["NG Tip", stats?.tipNg, "red"],
  ] as const;
  const pointColumns: ColumnsType<PointSummaryStatistics> = [
    { title: "Điểm đo", width: 165, render: (_, point) => `${point.station} · P${point.pointNumber} (${formatDecimal(point.position)} mm)` },
    { title: "Lực đè TB (g)", dataIndex: "loadAvg", render: numeric, align: "right" },
    { title: "K/c đẩy TB (mm)", dataIndex: "pushDistAvg", render: numeric, align: "right" },
    { title: "Độ cong Min (mm)", dataIndex: "bendMin", render: numeric, align: "right" },
    { title: "Độ cong TB (mm)", dataIndex: "bendAvg", render: numeric, align: "right" },
    { title: "Độ cong Max (mm)", dataIndex: "bendMax", render: numeric, align: "right" },
    { title: "Đo lại", dataIndex: "retested", width: 65, align: "right" },
    { title: "NG", dataIndex: "ng", width: 55, align: "right", className: "reports-point-ng" },
  ];
  const columns: ColumnsType<BackendMeasurement> = [
    { title: "Thời gian nhận", dataIndex: "measuredAt", render: formatReportTime, width: 165 },
    { title: "SKU", dataIndex: "productCode", width: 150 },
    { title: "Số gậy", dataIndex: "clubNumber", width: 80 },
    { title: "RecordSeq", dataIndex: "recordSeq", width: 100 },
    { title: "BUTT Load (g)", dataIndex: "buttLoad", render: formatDecimal, width: 125 },
    { title: "TIP Load (g)", dataIndex: "tipLoad", render: formatDecimal, width: 125 },
    ...([ ["Butt", "buttResult"], ["Tip", "tipResult"], ["Overall", "overallResult"] ] as const).map(([title, dataIndex]) => ({
      title, dataIndex, width: 90, render: (value: BackendMeasurement["buttResult"]) => <ResultTag result={value} />,
    })),
  ];
  const empty = loading ? <div className="reports-loading"><Spin /></div>
    : <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={error ? "Chưa tải được dữ liệu" : "Chưa có dữ liệu phù hợp"} />;

  return <div className="reports-page">
    {contextHolder}
    <PageHeader eyebrow="Chất lượng" title="3 · Báo cáo" description="Phân tích kết quả và dữ liệu đo đã lưu trong PostgreSQL." />
    <Card className="section-card reports-filter-card" title="Bộ lọc báo cáo">
      <div className="reports-filter-row">
      <div className="reports-presets"><span>Thời gian</span><Space wrap size={6}>
        {([ ["today", "Hôm nay"], ["week", "7 ngày"], ["custom", "Tùy chọn"] ] as const).map(([value, label]) => (
          <Button key={value} type={period === value ? "primary" : "default"} onClick={() => {
            setPeriod(value); if (value === "today") setRange([vietnamNow(), vietnamNow()]); else if (value === "week") setRange(defaultRange());
          }}>{label}</Button>
        ))}
      </Space></div>
        <label><span>Từ ngày</span><DatePicker format="DD/MM/YYYY" value={range[0]} allowClear={false} onChange={date => { if (date) { setRange([date, range[1]]); setPeriod("custom"); } }} /></label>
        <label><span>Đến ngày</span><DatePicker format="DD/MM/YYYY" value={range[1]} allowClear={false} onChange={date => { if (date) { setRange([range[0], date]); setPeriod("custom"); } }} /></label>
        <label><span>SKU</span><Select showSearch filterOption={false} value={product || undefined} allowClear placeholder="Tất cả SKU"
          loading={skuLoading} disabled={!databaseConnected} onChange={value => setProduct(value ?? "")}
          onSearch={text => { setSkuOptions([]); setSkuHasMore(false); setSkuSearch({ text, page: 1 }); }}
          onOpenChange={open => { if (open && (skuSearch.text || skuError)) { setSkuOptions([]); setSkuSearch({ text: "", page: 1 }); } }}
          onPopupScroll={event => {
            const target = event.currentTarget;
            if (skuHasMore && !skuLoading && !skuRequestPending.current && target.scrollTop + target.clientHeight >= target.scrollHeight - 24) {
              skuRequestPending.current = true; setSkuSearch(current => ({ ...current, page: current.page + 1 }));
            }
          }}
          options={skuOptions.map(value => ({ value, label: value }))}
          notFoundContent={skuLoading ? <Spin size="small" /> : skuError ? <div>{skuError}<Button type="link" onClick={() => setSkuSearch(current => ({ ...current, page: 1 }))}>Thử lại</Button></div> : "Không có SKU phù hợp"} /></label>
        <label><span>Số gậy</span><Input value={club} onChange={event => setClub(event.target.value)} placeholder="Tất cả" allowClear /></label>
      </div>
      <div className="reports-filter-actions">
        <div className="reports-result-filters">
        {([ ["Overall", overall, setOverall], ["Butt", butt, setButt], ["Tip", tip, setTip] ] as const).map(([label, value, setter]) => (
          <label key={label}><span>Kết quả {label}</span><Select value={value} onChange={setter} allowClear placeholder="Tất cả"
            options={[{ value: "OK", label: "OK" }, { value: "NG", label: "NG" }, { value: "UNMEASURED", label: "Chưa đo" }]} /></label>
        ))}
        </div>
        <Space wrap className="reports-main-actions"><Button className="reports-apply" type="primary" icon={<FilterOutlined />} onClick={apply} loading={loading} disabled={!databaseConnected || exporting}>Tạo báo cáo</Button>
          <Button className="reports-reset" icon={<ReloadOutlined />} onClick={reset} disabled={exporting}>Đặt lại</Button></Space>
        <Space wrap className="reports-export-actions"><Button className="reports-excel" icon={<FileExcelOutlined />} loading={exporting} disabled={!databaseConnected || loading || !stats || exporting} onClick={() => void exportReport("xlsx")}>Xuất Excel</Button>
          <Button className="reports-csv" icon={<FileTextOutlined />} loading={exporting} disabled={!databaseConnected || loading || !stats || exporting} onClick={() => void exportReport("csv")}>Xuất CSV</Button></Space>
      </div>
    </Card>
    {error && <Alert type="error" showIcon title={error} />}
    <div className="report-stat-grid">{cards.map(([label, value, tone]) => <Card key={label} className={`report-stat report-stat-${tone}`}>
      <span>{label}</span><strong>{loading ? <Spin size="small" /> : typeof value === "number" ? value.toLocaleString("vi-VN") : value ?? "—"}</strong>
    </Card>)}</div>
    {quality && stats && <div className="reports-extra-stats">
      <Card><span>Điểm có nhiều NG nhất</span><strong>{mostNg ? `${mostNg.label}: ${mostNg.ng} gậy` : "Chưa có điểm NG"}</strong></Card>
      <Card><span>Gậy đo lại (ít nhất một điểm có ≥ 2 lượt)</span><strong>{quality.retestedClubs.toLocaleString("vi-VN")} · {stats.totalMeasurements ? (100 * quality.retestedClubs / stats.totalMeasurements).toLocaleString("vi-VN", { maximumFractionDigits: 1 }) : "0"}%</strong></Card>
    </div>}
    <div className="reports-chart-grid">
      <Card className="section-card" title={`Số lượng OK / NG theo ${timeUnit === "month" ? "tháng" : timeUnit === "week" ? "tuần (từ Chủ nhật)" : "ngày"}`}>
        {grouped.size ? <div className="report-chart"><ResponsiveContainer width="100%" height="100%"><BarChart data={[...grouped.values()]}>
          <CartesianGrid vertical={false} stroke="#edf2f7" /><XAxis dataKey="date" tick={{ fontSize: 13 }} tickFormatter={value => dayjs(value).format("DD/MM")} /><YAxis allowDecimals={false} width={45} />
          <Tooltip /><Legend /><Bar dataKey="ok" name="OK (gậy)" fill="#23844b" /><Bar dataKey="ng" name="NG (gậy)" fill="#cf3c48" />
        </BarChart></ResponsiveContainer></div> : empty}
      </Card>
      <Card className="section-card" title="NG theo trạm và điểm đo">
        <div className="reports-stations">{stats?.stationResults.map(station => <div key={station.station}>
          <b>{station.station.toUpperCase()}</b><span>{station.ng} gậy NG · {stats.totalMeasurements ? (station.ng * 100 / stats.totalMeasurements).toLocaleString("vi-VN", { maximumFractionDigits: 1 }) : "0"}% tổng gậy</span>
        </div>)}</div>
        {topNg.length ? <div className="report-chart report-ng-chart"><ResponsiveContainer width="100%" height="100%"><BarChart data={topNg} layout="vertical" margin={{ left: 0, right: 25 }}>
          <CartesianGrid horizontal={false} stroke="#edf2f7" /><XAxis type="number" allowDecimals={false} /><YAxis type="category" dataKey="label" width={115} tick={{ fontSize: 13 }} tickFormatter={value => value.length > 16 ? value.slice(0, 15) + "…" : value} />
          <Tooltip formatter={(value, _name, item) => [`${value} gậy · ${item.payload.rate.toLocaleString("vi-VN", { maximumFractionDigits: 1 })}% tại điểm này`, "NG"]} />
          <Bar dataKey="ng" fill="#cf3c48" />
        </BarChart></ResponsiveContainer></div> : empty}
      </Card>
      <Card className="section-card" title="Tỷ lệ OK theo ngày">
        {daily.length ? <div className="report-chart"><ResponsiveContainer width="100%" height="100%"><LineChart data={daily}>
          <CartesianGrid vertical={false} stroke="#edf2f7" /><XAxis dataKey="date" tick={{ fontSize: 13 }} tickFormatter={value => dayjs(value).format("DD/MM")} /><YAxis domain={[0, 100]} unit="%" width={50} />
          <Tooltip content={({ active, payload }) => {
            const entry = payload?.[0]?.payload;
            return active && entry ? <div className="report-tooltip"><b>{entry.date}</b><div>Tổng: {entry.total} · OK: {entry.ok} · NG: {entry.ng}</div><div>Tỷ lệ OK: {entry.rate?.toLocaleString("vi-VN", { maximumFractionDigits: 2 })}%</div></div> : null;
          }} />
          <Line dataKey="rate" name="Tỷ lệ OK" stroke="#1769aa" strokeWidth={2} dot={daily.length < 40} isAnimationActive={false} />
        </LineChart></ResponsiveContainer></div> : empty}
      </Card>
    </div>
    <div className="reports-detail-grid">
      <Card className="section-card" title="Phân bố độ cong">
        <Select className="reports-point-picker" placeholder="Chọn SKU / trạm / điểm / vị trí" value={selectedPoint} onChange={setSelectedPoint}
          options={points.filter(point => point.bendCount > 0).map(point => ({ value: pointKey(point), label: pointLabel(point) }))} />
        {histogram.length ? <>
          <div className="report-chart"><ResponsiveContainer width="100%" height="100%"><BarChart data={histogram}>
            <CartesianGrid vertical={false} stroke="#edf2f7" /><XAxis dataKey="label" tick={{ fontSize: 13 }} interval="preserveStartEnd" label={{ value: "Độ cong (mm)", position: "insideBottom", offset: -5 }} height={50} />
            <YAxis allowDecimals={false} width={40} /><Tooltip labelFormatter={(_label, payload) => payload?.[0]?.payload.bounds ?? ""} /><Bar dataKey="count" name="Số gậy" fill="#3987c9" />
          </BarChart></ResponsiveContainer></div>
        </> : empty}
      </Card>
    <Card className="section-card" title="Tổng hợp theo điểm đo">
      <Table<PointSummaryStatistics> key={JSON.stringify(query.filters)} className="reports-point-summary" tableLayout="fixed"
        rowKey={point => JSON.stringify([point.station, point.pointNumber, point.position])}
        columns={pointColumns} dataSource={quality?.pointSummary ?? []} loading={loading} size="small" scroll={{ x: 680 }}
        pagination={{ pageSize: 10, showSizeChanger: false, showTotal: value => `${value} nhóm điểm đo` }} locale={{ emptyText: empty }} />
    </Card>
    </div>
    <Card className="section-card" title="Phép đo trong báo cáo">
      {listError && <Alert type="error" showIcon title={listError} />}
      <Table<BackendMeasurement> className="reports-table" rowKey="id" columns={columns} dataSource={records} loading={listLoading} scroll={{ x: 1050 }}
        pagination={{ current: query.page, pageSize: 10, total, showSizeChanger: false, showTotal: value => `${value} phép đo`, onChange: page => setQuery(current => ({ ...current, page })) }}
        locale={{ emptyText: empty }} onRow={record => ({ onClick: () => navigate(`/measurements/${record.id}`), style: { cursor: "pointer" } })} />
    </Card>
  </div>;
}
export default Reports;
