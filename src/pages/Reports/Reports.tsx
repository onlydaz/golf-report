import { FileExcelOutlined, FileTextOutlined, FilterOutlined, ReloadOutlined } from "@ant-design/icons";
import { Alert, Button, Card, DatePicker, Empty, Input, Pagination, Select, Space, Spin, Table, message } from "antd";
import type { ColumnsType } from "antd/es/table";
import type { Dayjs } from "dayjs";
import dayjs from "dayjs";
import { save } from "@tauri-apps/plugin-dialog";
import { writeFile } from "@tauri-apps/plugin-fs";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  Bar, BarChart, CartesianGrid, Cell, LabelList, Line, LineChart, ReferenceLine, ResponsiveContainer, Tooltip, XAxis, YAxis,
} from "recharts";
import { useNavigate } from "react-router-dom";
import PageHeader from "../../components/PageHeader";
import { ResultTag } from "../../components/measurement/MeasurementDetails";
import { useDatabaseConnected } from "../../hooks/usePlcMonitor";
import {
  getMeasurementStatistics, getMeasurementQualityStatistics, getMeasurementReport, getMeasurements, getMeasurementSkus,
  type BackendMeasurement, type MeasurementListRequest, type MeasurementStatistics,
  type MeasurementQualityStatistics, type PointSummaryStatistics,
} from "../../services/measurementService";
import { createCsvBytes, createExcelBytes } from "../../utils/as200Export";
import { buildReportTables, formatDecimal, formatLoadKg, isBusinessPoint } from "../../utils/as200Report";
import { formatReportTime, reportDateBoundary, REPORT_TIME_ZONE, vietnamNow } from "../../utils/reportTime";
import { groupedReportWindow } from "../../utils/reportBuckets";
import { measurementSettings } from "../../config/measurementSettings";
import { bendDomain, bendIntervalZone } from "../../utils/bendChart";
import "./Reports.css";

type ResultFilter = "OK" | "NG" | "UNMEASURED" | undefined;
type Period = "today" | "week" | "custom";
const defaultRange = (): [Dayjs, Dayjs] => [vietnamNow().subtract(6, "day"), vietnamNow()];
const numeric = (value: string | null) => value == null || !Number.isFinite(Number(value))
  ? "—" : Number(value).toLocaleString("vi-VN", { maximumSignificantDigits: 9 });

const pointColumns: ColumnsType<PointSummaryStatistics> = [
  { title: "Điểm đo", width: 145, render: (_, point) => `${point.station} · P${point.pointNumber} (${formatDecimal(point.position)} mm)` },
  { title: "Lực đè TB (kg)", dataIndex: "loadAvg", width: 120, render: formatLoadKg, align: "right" },
  { title: "K/c đẩy TB (mm)", dataIndex: "pushDistAvg", render: numeric, align: "right" },
  { title: "Độ cong Min (mm)", dataIndex: "bendMin", render: numeric, align: "right" },
  { title: "Độ cong TB (mm)", dataIndex: "bendAvg", render: numeric, align: "right" },
  { title: "Độ cong Max (mm)", dataIndex: "bendMax", render: numeric, align: "right" },
  { title: "Đo lại", dataIndex: "retested", width: 90, align: "center" },
  { title: "NG", dataIndex: "ng", width: 80, align: "center", className: "reports-point-ng" },
];
const columns: ColumnsType<BackendMeasurement> = [
  { title: "Thời gian nhận", dataIndex: "measuredAt", render: formatReportTime, width: 165 },
  { title: "SKU", dataIndex: "productCode", width: 150 },
  { title: "Số gậy", dataIndex: "clubNumber", width: 80 },
  { title: "BUTT Load (kg)", dataIndex: "buttLoad", render: formatLoadKg, width: 125 },
  { title: "TIP Load (kg)", dataIndex: "tipLoad", render: formatLoadKg, width: 125 },
  ...([ ["Butt", "buttResult"], ["Tip", "tipResult"], ["Overall", "overallResult"] ] as const).map(([title, dataIndex]) => ({
    title, dataIndex, width: 90, render: (value: BackendMeasurement["buttResult"]) => <ResultTag result={value} />,
  })),
];

function Reports() {
  const navigate = useNavigate();
  const databaseConnected = useDatabaseConnected();
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
  const filterKey = JSON.stringify(query.filters);
  const listKey = JSON.stringify(query);
  const [summary, setSummary] = useState<{ key: string; stats: MeasurementStatistics; quality: MeasurementQualityStatistics }>();
  const [list, setList] = useState<{ key: string; items: BackendMeasurement[]; total: number }>();
  const stats = summary?.key === filterKey ? summary.stats : undefined;
  const quality = summary?.key === filterKey ? summary.quality : undefined;
  const records = list?.key === listKey ? list.items : [];
  const total = list?.key === listKey ? list.total : 0;
  const [chartWindow, setChartWindow] = useState<{ key: string; page: number }>();
  const buckets = useMemo(() => groupedReportWindow(quality?.halfHour, query.filters.measuredFrom, query.filters.measuredTo,
    chartWindow?.key === filterKey ? chartWindow.page : undefined),
  [quality?.halfHour, query.filters.measuredFrom, query.filters.measuredTo, chartWindow, filterKey]);
  const halfHour = buckets.items;
  const bucketTitle = buckets.grouping === "halfHour" ? "30 phút" : buckets.grouping === "day" ? "ngày" : "tuần";
  const [loading, setLoading] = useState(true);
  const [listLoading, setListLoading] = useState(true);
  const [error, setError] = useState("");
  const [listError, setListError] = useState("");
  const [exporting, setExporting] = useState(false);

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
    setError("");
    setLoading(databaseConnected);
    if (!databaseConnected) return;
    void Promise.all([getMeasurementStatistics(query.filters), getMeasurementQualityStatistics(query.filters)])
      .then(([summary, points]) => {
        if (!active) return;
        setSummary({ key: filterKey, stats: summary, quality: points });
      }).catch(() => { if (active) setError("Không thể tải báo cáo chất lượng. Vui lòng tạo báo cáo lại."); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [query.filters, databaseConnected]);

  useEffect(() => {
    let active = true;
    setListError(""); setListLoading(databaseConnected);
    if (!databaseConnected) return;
    void getMeasurements({ ...query.filters, page: query.page, pageSize: 10 }).then(response => {
      if (!active) return;
      const lastPage = Math.max(1, Math.ceil(response.total / 10));
      if (query.page > lastPage) { setQuery(current => ({ ...current, page: lastPage })); return; }
      setList({ key: listKey, items: response.items, total: response.total });
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

  const bendSettings = measurementSettings.BUTT;
  const { mostNg, daily, topNg, cpk, histogram, histogramDomain, pointSummary } = useMemo(() => {
    const points = (quality?.points ?? []).filter(point => isBusinessPoint(point.station, point.pointNumber));
    // Each measurement belongs to one SKU/position at a given station/point.
    // Combine SKU/position groups by station/point; each record contributes once per point.
    const ngByPoint = new Map<string, { station: string; pointNumber: number; ng: number }>();
    for (const point of points) {
      const key = `${point.station}-${point.pointNumber}`;
      const entry = ngByPoint.get(key) ?? { station: point.station, pointNumber: point.pointNumber, ng: 0 };
      entry.ng += point.ng;
      ngByPoint.set(key, entry);
    }
    const mostNg = stats?.totalNg ? [...ngByPoint.values()].filter(point => point.ng > 0).sort((a, b) => b.ng - a.ng)[0] : undefined;
    const daily = stats?.daily.map(item => ({ ...item, rate: item.total ? 100 * item.ok / item.total : null })) ?? [];
    const topNg = quality && stats ? ([
      ["BUTT", 1, 150], ["BUTT", 2, 300], ["BUTT", 3, 450], ["BUTT", 4, 600], ["TIP", 1, 80],
    ] as const).map(([station, pointNumber, demoPosition]) => {
      const samples = points.filter(point => point.station === station && point.pointNumber === pointNumber);
      const ng = samples.reduce((sum, point) => sum + point.ng, 0);
      const positions = [...new Set(samples.flatMap(point => point.position == null ? [] : [point.position]))]
        .sort((a, b) => Number(a) - Number(b));
      const position = positions.length ? positions.map(formatDecimal).join(" · ") : quality.demoLimits ? String(demoPosition) : "—";
      const rate = stats.totalMeasurements ? 100 * ng / stats.totalMeasurements : 0;
      return { station, pointNumber, ng, rate, position, positionLabel: positions.length > 1 ? `${positions.length} vị trí` : `${position} mm`,
        label: `${station} · P${pointNumber}`, annotation: `${ng} (${rate.toLocaleString("vi-VN", { maximumFractionDigits: 1 })}%)` };
    }).sort((a, b) => b.ng - a.ng) : [];
    const cpk = (() => {
      const summary = quality?.buttSummary;
      const lower = bendSettings.bendLowerMm, upper = bendSettings.bendUpperMm;
      if (lower == null || upper == null || !summary || summary.count < 2 || summary.mean == null || summary.stddev == null) return null;
      const mean = Number(summary.mean), deviation = Number(summary.stddev);
      if (![lower, upper, mean, deviation].every(Number.isFinite) || upper <= lower || deviation <= 0) return null;
      return Math.min((upper - mean) / (3 * deviation), (mean - lower) / (3 * deviation));
    })();
    const histogram = quality?.buttHistogram?.map((bin, index) => ({ ...bin,
      zone: bendIntervalZone(Number(bin.lower), Number(bin.upper), index === quality.buttHistogram.length - 1, bendSettings),
      midpoint: (Number(bin.lower) + Number(bin.upper)) / 2,
      label: bin.lower === bin.upper ? numeric(bin.lower) : `${numeric(bin.lower)}–${numeric(bin.upper)}`,
      bounds: `[${formatDecimal(bin.lower)}; ${formatDecimal(bin.upper)}${index === quality.buttHistogram.length - 1 ? "]" : ")"} mm`,
    })) ?? [];
    const histogramDomain = bendDomain(histogram.flatMap(bin => [Number(bin.lower), Number(bin.upper)]), bendSettings);
    const pointSummary = (quality?.pointSummary ?? []).filter(point => isBusinessPoint(point.station, point.pointNumber));
    return { mostNg, daily, topNg, cpk, histogram, histogramDomain, pointSummary };
  }, [quality, stats, bendSettings]);
  const cards = [
    ["Tổng số gậy", stats?.totalMeasurements, "blue", undefined],
    ["Tổng OK", stats?.totalOk, "green", stats
      ? `Tỷ lệ OK ${stats.passRate.toLocaleString("vi-VN", { minimumFractionDigits: 1, maximumFractionDigits: 1 })}%` : "Tỷ lệ OK —"],
    ["Tổng NG", stats?.totalNg, "red", stats
      ? `Tỷ lệ NG ${(stats.totalMeasurements ? 100 * stats.totalNg / stats.totalMeasurements : 0).toLocaleString("vi-VN", { minimumFractionDigits: 1, maximumFractionDigits: 1 })}%` : "Tỷ lệ NG —"],
  ] as const;
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
    <div className="report-stat-grid">{cards.map(([label, value, tone, subtitle]) => <Card key={label} className={`report-stat report-stat-${tone}`}>
      <span>{label}</span><strong>{loading && !stats ? <Spin size="small" /> : typeof value === "number" ? value.toLocaleString("vi-VN") : value ?? "—"}</strong>
      {subtitle && <small>{subtitle}</small>}
    </Card>)}
      <Card className="report-stat"><span>NG nhiều nhất tại</span>
        <strong>{loading && !stats ? <Spin size="small" /> : quality && stats ? mostNg ? `${mostNg.station} · P${mostNg.pointNumber}` : "Chưa có" : "—"}</strong>
        <small>{quality && stats ? <>{mostNg?.ng.toLocaleString("vi-VN") ?? "0"} gậy · {mostNg && stats.totalNg ? (100 * mostNg.ng / stats.totalNg).toLocaleString("vi-VN", { maximumFractionDigits: 1 }) : "0"}% tổng NG</> : "—"}</small>
      </Card>
      <Card className="report-stat"><span>Gậy phải đo lại (≥ 2 lần)</span>
        <strong>{loading && !stats ? <Spin size="small" /> : quality && stats ? stats.totalMeasurements ? quality.retestedClubs.toLocaleString("vi-VN") : "0" : "—"}</strong>
        <small>{quality && stats ? <>{stats.totalMeasurements ? (100 * quality.retestedClubs / stats.totalMeasurements).toLocaleString("vi-VN", { maximumFractionDigits: 1 }) : "0"}% tổng gậy</> : "—"}</small>
      </Card>
    </div>
    <div className="reports-chart-grid">
      <Card className="section-card reports-half-hour-card" title={`Số lượng OK / NG theo ${bucketTitle}`}
        extra={<div className="reports-half-hour-legend" aria-label="Chú giải kết quả">
          <span><i className="reports-half-hour-ok" />OK</span><span><i className="reports-half-hour-ng" />NG</span>
        </div>}>
        {halfHour.length > 0 && <div className="reports-half-hour-window">
          <small>{dayjs(halfHour[0].from).tz(REPORT_TIME_ZONE).format("DD/MM/YYYY HH:mm")} – {dayjs(halfHour[halfHour.length - 1].to).tz(REPORT_TIME_ZONE).format("DD/MM/YYYY HH:mm")}</small>
          {buckets.pages > 1 && <Pagination size="small" simple current={buckets.current} total={buckets.pages} pageSize={1}
            onChange={page => setChartWindow({ key: filterKey, page })} />}
        </div>}
        {halfHour.length ? <div className="report-chart"><ResponsiveContainer width="100%" height="100%"><BarChart key={`${filterKey}-${buckets.current}`} data={halfHour}
          barSize={32} barCategoryGap="18%" margin={{ top: 26, right: 12, bottom: 4, left: 0 }}>
          <CartesianGrid vertical={false} stroke="#edf2f7" />
          <XAxis type="category" dataKey="at" tick={{ fontSize: 12 }} tickLine={false} interval={0}
            tickFormatter={value => dayjs(value).tz(REPORT_TIME_ZONE).format(buckets.grouping === "halfHour" ? "HH:mm" : "DD/MM")} />
          <YAxis domain={[0, "auto"]} allowDecimals={false} width={40} tickLine={false} axisLine={false} />
          <Tooltip cursor={{ fill: "#f1f5f9" }} content={({ active, payload }) => {
            const bucket = payload?.[0]?.payload;
            return active && bucket ? <div className="report-tooltip">
              <b>{dayjs(bucket.from).tz(REPORT_TIME_ZONE).format("DD/MM/YYYY HH:mm")} – {dayjs(bucket.to).tz(REPORT_TIME_ZONE).format("DD/MM/YYYY HH:mm")}</b>
              <div style={{ color: "#15803D" }}>OK: {bucket.ok}</div>
              <div style={{ color: "#B91C1C" }}>NG: {bucket.ng}</div>
              <div>Tổng: {bucket.ok + bucket.ng} gậy</div>
            </div> : null;
          }} />
          <Bar isAnimationActive={false} dataKey="ok" stackId="quality" name="OK" fill="#15803D" maxBarSize={40} />
          <Bar isAnimationActive={false} dataKey="ng" stackId="quality" name="NG" fill="#B91C1C" maxBarSize={40}>
            <LabelList dataKey="ng" position="top" fill="#B91C1C" fontSize={12} fontWeight={600}
              formatter={value => typeof value === "number" && value > 0 ? value.toLocaleString("vi-VN") : ""} />
          </Bar>
        </BarChart></ResponsiveContainer></div> : empty}
      </Card>
      <Card className="section-card" title="NG theo công đoạn / điểm đo">
        {topNg.length ? <div className="report-ng-progress-list">
          {topNg.map(point => <div className="report-ng-progress-row" key={point.label}>
            <span className="report-ng-progress-label">{point.label}</span>
            <div className="report-ng-progress-track" role="meter" aria-label={`Số gậy NG tại ${point.label}`}
              aria-valuemin={0} aria-valuemax={Math.max(1, topNg[0].ng)} aria-valuenow={point.ng}
              aria-valuetext={`${point.ng} gậy NG · ${point.rate.toLocaleString("vi-VN", { maximumFractionDigits: 1 })}%`}>
              <div className="report-ng-progress-fill" style={{
                width: `${topNg[0].ng > 0 ? 100 * point.ng / topNg[0].ng : 0}%`,
                backgroundColor: point.station === "BUTT" ? "#2878D0" : "#F0642C",
              }} />
            </div>
            <span className="report-ng-progress-value">{point.ng.toLocaleString("vi-VN")} · {point.rate.toLocaleString("vi-VN", { maximumFractionDigits: 1 })}%</span>
          </div>)}
        </div> : empty}
      </Card>
      <Card className="section-card" title="Tỷ lệ OK theo ngày">
        {daily.length ? <div className="report-chart"><ResponsiveContainer width="100%" height="100%"><LineChart data={daily} margin={{ top: 26, right: 24, bottom: 4, left: 8 }}>
          <CartesianGrid vertical={false} stroke="#edf2f7" /><XAxis dataKey="date" tick={{ fontSize: 13 }} tickFormatter={value => dayjs(value).format("DD/MM")} />
          <YAxis domain={[75, 100]} ticks={[75, 80, 85, 90, 95, 100]} interval={0} allowDataOverflow
            tickFormatter={value => `${value}%`} width={50} />
          <ReferenceLine y={95} stroke="#94a3b8" strokeDasharray="5 4"
            label={{ value: "Mục tiêu 95%", position: "insideTopRight", fill: "#64748b", fontSize: 12 }} />
          <Tooltip content={({ active, payload }) => {
            const entry = payload?.[0]?.payload;
            return active && entry ? <div className="report-tooltip"><b>{entry.date}</b><div>Tổng: {entry.total} · OK: {entry.ok} · NG: {entry.ng}</div><div>Tỷ lệ OK: {entry.rate?.toLocaleString("vi-VN", { maximumFractionDigits: 2 })}%</div></div> : null;
          }} />
          <Line dataKey="rate" name="Tỷ lệ OK" stroke="#1769aa" strokeWidth={2} isAnimationActive={false}
            dot={({ cx, cy, payload }) => cx == null || cy == null || payload?.rate == null ? <g />
              : <circle cx={cx} cy={cy} r={4} fill={payload.rate < 95 ? "#B91C1C" : "#1769aa"} stroke="white" strokeWidth={1.5} />}
            activeDot={({ cx, cy, payload }) => cx == null || cy == null || payload?.rate == null ? <g />
              : <circle cx={cx} cy={cy} r={6} fill={payload.rate < 95 ? "#B91C1C" : "#1769aa"} stroke="white" strokeWidth={2} />}>
            <LabelList dataKey="rate" content={({ x, y, value }) => typeof value !== "number" || x == null || y == null ? <g />
              : <text x={Number(x)} y={Number(y) - 10} textAnchor="middle" fontSize={12} fontWeight={600}
                fill={value < 95 ? "#B91C1C" : "#1769aa"}>
                {value.toLocaleString("vi-VN", { minimumFractionDigits: 1, maximumFractionDigits: 1 })}%
              </text>} />
          </Line>
        </LineChart></ResponsiveContainer></div> : empty}
      </Card>
    </div>
    <div className="reports-detail-grid">
      <Card className="section-card" title="Phân bố độ cong BUTT (tất cả điểm)">
        {histogram.length ? <>
          <div className="report-chart"><ResponsiveContainer width="100%" height="100%"><BarChart data={histogram} margin={{ top: 12, right: 16, bottom: 8, left: 20 }}>
            <CartesianGrid vertical={false} stroke="#edf2f7" />
            <XAxis type="number" dataKey="midpoint" domain={histogramDomain} tick={{ fontSize: 13 }}
              tickFormatter={value => Number(value).toLocaleString("vi-VN", { maximumFractionDigits: 2 })}
              label={{ value: "Độ cong (mm)", position: "insideBottom", offset: -5 }} height={50} />
            <YAxis allowDecimals={false} width={90} tickMargin={8}
              label={{ value: "Số phép đo", angle: -90, position: "insideLeft", offset: 10, textAnchor: "middle", fontSize: 13 }} />
            {bendSettings.bendLowerMm != null && <ReferenceLine x={bendSettings.bendLowerMm} stroke="#b42318" strokeDasharray="4 4" ifOverflow="extendDomain"
              label={{ value: `${formatDecimal(String(bendSettings.bendLowerMm))} mm`, position: "insideTopRight", fontSize: 13 }} />}
            {bendSettings.bendUpperMm != null && <ReferenceLine x={bendSettings.bendUpperMm} stroke="#b42318" strokeDasharray="4 4" ifOverflow="extendDomain"
              label={{ value: `${formatDecimal(String(bendSettings.bendUpperMm))} mm`, position: "insideTopLeft", fontSize: 13 }} />}
            <Tooltip content={({ active, payload }) => {
              const bin = payload?.[0]?.payload;
              return active && bin ? <div className="report-tooltip"><b>{bin.bounds}</b><div>{bin.count} phép đo</div>
                <div>Giới hạn: {formatDecimal(bendSettings.bendLowerMm?.toString())} – {formatDecimal(bendSettings.bendUpperMm?.toString())} mm</div>
                <div>{bin.zone === "WITHIN" ? "Trong giới hạn" : bin.zone === "MIXED" ? "Nhóm giao giới hạn; không xác định số mẫu vượt từ dữ liệu tổng hợp"
                  : bin.zone === "UNKNOWN" ? "Chưa có giới hạn" : "Vượt giới hạn"}</div>
              </div> : null;
            }} />
            <Bar isAnimationActive={false} dataKey="count" name="Số phép đo" fill="#3987c9" maxBarSize={40}>
              {histogram.map((bin, index) => <Cell key={index} fill={bin.zone === "BELOW" || bin.zone === "ABOVE" ? "#cf3c48" : bin.zone === "MIXED" ? "#d99222" : "#3987c9"} />)}
            </Bar>
          </BarChart></ResponsiveContainer></div>
          {quality?.buttSummary && <small className="report-distribution-summary">
            Trung bình: {numeric(quality.buttSummary.mean)} mm · Độ lệch chuẩn mẫu: {numeric(quality.buttSummary.stddev)} mm
            {` · Cpk tham chiếu: ${cpk == null ? "—" : cpk.toLocaleString("vi-VN", { maximumFractionDigits: 3 })}`}
          </small>}
        </> : empty}
      </Card>
    <Card className="section-card" title="Tổng hợp theo điểm đo">
      <Table<PointSummaryStatistics> key={JSON.stringify(query.filters)} className="reports-point-summary" tableLayout="fixed"
        rowKey={point => JSON.stringify([point.station, point.pointNumber, point.position])}
        columns={pointColumns} dataSource={pointSummary} loading={loading} size="small" scroll={{ x: 1080 }}
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
