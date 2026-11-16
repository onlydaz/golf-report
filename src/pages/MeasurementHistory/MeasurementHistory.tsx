import { EyeOutlined, ReloadOutlined, SearchOutlined } from "@ant-design/icons";
import { Alert, Button, Card, DatePicker, Empty, InputNumber, Select, Spin, Table } from "antd";
import type { ColumnsType } from "antd/es/table";
import type { Dayjs } from "dayjs";
import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { ResultTag } from "../../components/measurement/MeasurementDetails";
import PageHeader from "../../components/PageHeader";
import { usePlcMonitor } from "../../hooks/usePlcMonitor";
import {
  getMeasurementHistoryPage, getMeasurementSkus, getMeasurementSkuSummary, getMeasurementStatistics,
  type MeasurementHistoryItem, type MeasurementHistoryPage, type MeasurementListRequest,
  type MeasurementSkuPage, type MeasurementSkuSummary, type MeasurementStatistics,
} from "../../services/measurementService";
import { formatDecimal } from "../../utils/as200Report";
import { formatReportTime, reportDateBoundary } from "../../utils/reportTime";
import "./MeasurementHistory.css";

type Result = "OK" | "NG";
interface Filters { from?: Dayjs; to?: Dayjs; productCode?: string; clubNumber?: number; result?: Result }
const pageSize = 10;
const rate = (ok: number, total: number) => `${(total ? ok * 100 / total : 0).toLocaleString("vi-VN", { maximumFractionDigits: 1 })}%`;

function MeasurementHistory() {
  const navigate = useNavigate();
  const { databaseConnected, status } = usePlcMonitor();
  const [draft, setDraft] = useState<Filters>({});
  const [query, setQuery] = useState<{ filters: MeasurementListRequest; page: number }>({ filters: {}, page: 1 });
  const [reload, setReload] = useState(0);
  const [summary, setSummary] = useState<{ query: typeof query; data: MeasurementSkuPage }>();
  const [selection, setSelection] = useState<{ scope: MeasurementListRequest; sku?: string; page: number }>({ scope: query.filters, page: 1 });
  const [details, setDetails] = useState<{ selection: typeof selection; data: MeasurementHistoryPage }>();
  const [statistics, setStatistics] = useState<{ scope: MeasurementListRequest; data: MeasurementStatistics }>();
  const [summaryLoading, setSummaryLoading] = useState(false);
  const [detailLoading, setDetailLoading] = useState(false);
  const [statisticsLoading, setStatisticsLoading] = useState(false);
  const [summaryError, setSummaryError] = useState("");
  const [detailError, setDetailError] = useState("");
  const [statisticsError, setStatisticsError] = useState("");
  const [filterError, setFilterError] = useState("");
  const [skuSearch, setSkuSearch] = useState({ text: "", page: 1 });
  const [skuOptions, setSkuOptions] = useState<string[]>([]);
  const [skuLoading, setSkuLoading] = useState(false);
  const [skuHasMore, setSkuHasMore] = useState(false);
  const [skuError, setSkuError] = useState("");
  const skuRequestPending = useRef(false);
  const selectedSku = selection.scope === query.filters ? selection.sku : undefined;

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
      }).catch(() => { if (active) { setSkuError("Không tải được danh sách SKU. Nhấn Tải lại để thử lại."); setSkuHasMore(false); } })
        .finally(() => { if (active) { setSkuLoading(false); skuRequestPending.current = false; } });
    }, 250);
    return () => { active = false; clearTimeout(timer); };
  }, [skuSearch, databaseConnected, reload]);

  useEffect(() => {
    let active = true;
    setSummary(undefined); setSummaryError(""); setSummaryLoading(databaseConnected);
    if (!databaseConnected) return;
    void getMeasurementSkuSummary({ ...query.filters, page: query.page, pageSize }).then(response => {
      if (!active) return;
      const lastPage = Math.max(1, Math.ceil(response.total / pageSize));
      if (query.page > lastPage) { setQuery(current => ({ ...current, page: lastPage })); return; }
      setSummary({ query, data: response });
      setSelection(previous => {
        const sku = previous.scope === query.filters && response.items.some(item => item.productCode === previous.sku)
          ? previous.sku : response.items[0]?.productCode;
        return previous.scope === query.filters && sku === previous.sku ? previous : { scope: query.filters, sku, page: 1 };
      });
    }).catch(() => { if (active) { setSummaryError("Không thể tải tổng hợp SKU. Vui lòng thử tải lại."); setSelection({ scope: query.filters, page: 1 }); } })
      .finally(() => { if (active) setSummaryLoading(false); });
    return () => { active = false; };
  }, [query, databaseConnected, status?.lastSavedId, reload]);

  useEffect(() => {
    let active = true;
    setStatistics(undefined); setStatisticsError(""); setStatisticsLoading(databaseConnected);
    if (!databaseConnected) return;
    void getMeasurementStatistics(query.filters).then(data => { if (active) setStatistics({ scope: query.filters, data }); })
      .catch(() => { if (active) setStatisticsError("Không thể tải thống kê. Vui lòng thử tải lại."); })
      .finally(() => { if (active) setStatisticsLoading(false); });
    return () => { active = false; };
  }, [query.filters, databaseConnected, status?.lastSavedId, reload]);

  useEffect(() => {
    let active = true;
    setDetails(undefined); setDetailError(""); setDetailLoading(databaseConnected && !!selectedSku);
    if (!databaseConnected || !selectedSku) return;
    void getMeasurementHistoryPage({ ...query.filters, productCode: selectedSku, page: selection.page, pageSize }).then(data => {
      if (!active) return;
      const lastPage = Math.max(1, Math.ceil(data.total / pageSize));
      if (selection.page > lastPage) { setSelection(current => ({ ...current, page: lastPage })); return; }
      setDetails({ selection, data });
    }).catch(() => { if (active) setDetailError("Không thể tải chi tiết gậy. Vui lòng thử tải lại."); })
      .finally(() => { if (active) setDetailLoading(false); });
    return () => { active = false; };
  }, [query.filters, selection, selectedSku, databaseConnected, status?.lastSavedId, reload]);

  const applyFilters = () => {
    if (draft.from && draft.to && draft.from.isAfter(draft.to, "day")) { setFilterError("Ngày bắt đầu phải trước hoặc bằng ngày kết thúc."); return; }
    setFilterError("");
    setQuery({ page: 1, filters: {
      productCode: draft.productCode, clubNumber: draft.clubNumber, result: draft.result,
      measuredFrom: draft.from ? reportDateBoundary(draft.from) : undefined,
      measuredTo: draft.to ? reportDateBoundary(draft.to, true) : undefined,
    } });
  };
  const stats = databaseConnected && statistics?.scope === query.filters ? statistics.data : undefined;
  const skuPage = databaseConnected && summary?.query === query ? summary.data : undefined;
  const detailPage = databaseConnected && details?.selection === selection && selectedSku ? details.data : undefined;
  const cards = [
    ["Tổng số gậy", stats?.totalMeasurements, "blue"], ["Tổng OK", stats?.totalOk, "green"], ["Tổng NG", stats?.totalNg, "red"],
    ["Tỷ lệ OK", stats ? rate(stats.totalOk, stats.totalMeasurements) : undefined, "green"], ["NG Butt", stats?.buttNg, "red"], ["NG Tip", stats?.tipNg, "red"],
  ] as const;
  const skuColumns: ColumnsType<MeasurementSkuSummary> = [
    { title: "SKU", dataIndex: "productCode", width: 160, fixed: "left" },
    { title: "Gậy đầu tiên", dataIndex: "firstMeasuredAt", render: formatReportTime, width: 180 },
    { title: "Gậy gần nhất", dataIndex: "lastMeasuredAt", render: formatReportTime, width: 180 },
    { title: "Tổng số gậy", dataIndex: "total", width: 110, align: "right" },
    { title: "OK", dataIndex: "ok", width: 85, align: "right", className: "history-value-ok" },
    { title: "NG", dataIndex: "ng", width: 85, align: "right", className: "history-value-ng" },
    { title: "NG BUTT", dataIndex: "buttNg", width: 100, align: "right" },
    { title: "NG TIP", dataIndex: "tipNg", width: 100, align: "right" },
    { title: "Tỷ lệ OK", render: (_, row) => rate(row.ok, row.total), width: 110, align: "right" },
  ];
  const stationColumns = (station: "BUTT" | "TIP"): ColumnsType<MeasurementHistoryItem> => [
    ...[1, 2, 3, 4].map(pointNumber => ({ title: `P${pointNumber} (mm)`, width: 112, align: "right" as const,
      render: (_: unknown, row: MeasurementHistoryItem) => formatDecimal(row.decisionPoints.find(point => point.station === station && point.pointNumber === pointNumber)?.bend),
    })),
    { title: `Lực ${station} (g)`, dataIndex: station === "BUTT" ? "buttLoad" : "tipLoad", render: formatDecimal, width: 135, align: "right" },
  ];
  const columns: ColumnsType<MeasurementHistoryItem> = [
    { title: <span title="Thời điểm phần mềm nhận bản ghi, giờ Việt Nam">Thời gian đo</span>, dataIndex: "measuredAt", width: 180, fixed: "left", render: formatReportTime },
    { title: "Số gậy", dataIndex: "clubNumber", width: 90, fixed: "left", align: "center" },
    { title: "BUTT", children: stationColumns("BUTT") }, { title: "TIP", children: stationColumns("TIP") },
    ...([ ["BUTT", "buttResult"], ["TIP", "tipResult"], ["Overall", "overallResult"] ] as const).map(([title, dataIndex]) => ({
      title, dataIndex, width: 95, align: "center" as const,
      render: (value: MeasurementHistoryItem["buttResult"]) => <ResultTag result={value} />,
    })),
    { title: "", key: "action", width: 140, render: (_, row) => <Button type="link" icon={<EyeOutlined />} onClick={() => navigate(`/measurements/${row.id}`)}>Xem chi tiết</Button> },
  ];
  const shownFrom = detailPage?.total ? (selection.page - 1) * pageSize + 1 : 0;
  const shownTo = detailPage ? Math.min(selection.page * pageSize, detailPage.total) : 0;
  const options = [...new Set([...(draft.productCode ? [draft.productCode] : []), ...skuOptions])]
    .filter(value => value.toLocaleLowerCase().includes(skuSearch.text.toLocaleLowerCase())).map(value => ({ value, label: value }));

  return <div className="measurement-history-page">
    <PageHeader eyebrow="Tra cứu" title="2 · Lịch sử" description="" />
    <Card className="section-card history-filter-card">
      <form className="history-filter-row" onSubmit={event => { event.preventDefault(); applyFilters(); }}>
        <label className="filter-field"><span>Từ ngày</span><DatePicker value={draft.from} format="DD/MM/YYYY" placeholder="Chọn ngày" onChange={value => setDraft(current => ({ ...current, from: value ?? undefined }))} /></label>
        <label className="filter-field"><span>Đến ngày</span><DatePicker value={draft.to} format="DD/MM/YYYY" placeholder="Chọn ngày" onChange={value => setDraft(current => ({ ...current, to: value ?? undefined }))} /></label>
        <label className="filter-field history-sku-select"><span>SKU</span><Select showSearch filterOption={false} allowClear placeholder="Tất cả SKU" value={draft.productCode} options={options} loading={skuLoading}
          onSearch={text => { setSkuOptions([]); setSkuSearch({ text, page: 1 }); }}
          onChange={value => setDraft(current => ({ ...current, productCode: value }))}
          onOpenChange={open => { if (open && skuSearch.text) { setSkuOptions([]); setSkuSearch({ text: "", page: 1 }); } }}
          onPopupScroll={event => { const node = event.currentTarget; if (skuHasMore && !skuLoading && !skuRequestPending.current && node.scrollTop + node.clientHeight >= node.scrollHeight - 12) { skuRequestPending.current = true; setSkuSearch(current => ({ ...current, page: current.page + 1 })); } }}
          notFoundContent={skuLoading ? <Spin size="small" /> : skuError || "Không có SKU phù hợp"} /></label>
        <label className="filter-field"><span>Kết quả Overall</span><Select<Result> placeholder="Tất cả" value={draft.result} allowClear options={[{ value: "OK", label: "OK" }, { value: "NG", label: "NG" }]} onChange={result => setDraft(current => ({ ...current, result }))} /></label>
        <label className="filter-field"><span>Tìm số gậy</span><InputNumber min={-2147483648} max={2147483647} precision={0} value={draft.clubNumber ?? null} placeholder="Số gậy" onChange={clubNumber => setDraft(current => ({ ...current, clubNumber: clubNumber ?? undefined }))} /></label>
        <div className="history-filter-actions"><Button type="primary" htmlType="submit" icon={<SearchOutlined />}>Lọc</Button><Button onClick={() => { setDraft({}); setFilterError(""); setQuery({ filters: {}, page: 1 }); }}>Đặt lại</Button></div>
      </form>
      {filterError && <Alert type="warning" showIcon title={filterError} />}
      {skuError && <Alert type="warning" showIcon title={skuError} />}
    </Card>
    <div className="history-section-heading"><span>Toàn bộ kết quả theo bộ lọc đã áp dụng</span><Button size="small" icon={<ReloadOutlined />} onClick={() => setReload(value => value + 1)} disabled={!databaseConnected || summaryLoading || detailLoading || statisticsLoading}>Tải lại</Button></div>
    {!databaseConnected && <Alert type="error" showIcon title="Chưa kết nối cơ sở dữ liệu. Hãy kết nối lại để tải lịch sử." />}
    {statisticsError && <Alert type="error" showIcon title={statisticsError} />}
    <section aria-label="Thống kê theo bộ lọc" className="history-statistics">{cards.map(([label, value, tone]) => <Card className={`history-stat history-stat-${tone}`} key={label}>
      <span className="history-stat-label">{label}</span><strong>{statisticsLoading ? <Spin size="small" /> : typeof value === "number" ? value.toLocaleString("vi-VN") : value ?? "—"}</strong>
    </Card>)}</section>
    <Card className="section-card" title="Các lần chạy theo SKU" extra={<span className="history-table-note">Hiển thị {skuPage?.items.length ?? 0} / {skuPage?.total ?? 0} SKU · Chọn một dòng để xem chi tiết từng gậy bên dưới</span>}>
      {summaryError && <Alert type="error" showIcon title={summaryError} />}
      <Table<MeasurementSkuSummary> className="history-table history-sku-table" rowKey="productCode" columns={skuColumns} dataSource={skuPage?.items ?? []} loading={summaryLoading}
        rowClassName={row => row.productCode === selectedSku ? "history-sku-selected" : ""}
        onRow={row => ({ onClick: () => setSelection({ scope: query.filters, sku: row.productCode, page: 1 }), tabIndex: 0, "aria-selected": row.productCode === selectedSku,
          onKeyDown: event => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); setSelection({ scope: query.filters, sku: row.productCode, page: 1 }); } }, style: { cursor: "pointer" } })}
        pagination={{ current: query.page, pageSize, total: skuPage?.total ?? 0, showSizeChanger: false, onChange: page => setQuery(current => ({ ...current, page })) }}
        scroll={{ x: 1110 }} locale={{ emptyText: <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={summaryError ? "Chưa tải được tổng hợp SKU" : "Không có SKU phù hợp"} /> }} />
    </Card>
    <Card className="section-card" title={selectedSku ? `Chi tiết gậy · ${selectedSku}` : "Chi tiết gậy"} extra={<span className="history-table-note">Hiển thị {shownFrom}–{shownTo} / {detailPage?.total ?? 0} gậy</span>}>
      {detailError && <Alert type="error" showIcon title={detailError} />}
      <Table<MeasurementHistoryItem> className="history-table history-detail-table" rowKey="id" columns={columns} dataSource={detailPage?.items ?? []} loading={detailLoading}
        pagination={{ current: selection.page, pageSize, total: detailPage?.total ?? 0, showSizeChanger: false, onChange: page => setSelection(current => ({ ...current, page })) }}
        scroll={{ x: 1880 }} locale={{ emptyText: <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={detailError ? "Chưa tải được chi tiết gậy" : selectedSku ? "Không có gậy phù hợp" : "Chọn một SKU để xem chi tiết gậy"} /> }} />
    </Card>
  </div>;
}
export default MeasurementHistory;
