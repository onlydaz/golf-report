import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ProductionSummary } from "./MonitorPanels";
import type { MeasurementStatistics } from "../../services/measurementService";
import type { MeasurementDetail } from "../../services/measurementService";
import { MemoryRouter } from "react-router-dom";
import MeasurementDetails from "../../components/measurement/MeasurementDetails";
import { RecentMeasurements } from "./MonitorPanels";

describe("monitor production totals across all ProductCodes", () => {
  it("renders N/A in stored detail and keeps the corresponding recent row without a ProductCode editor", () => {
    const data: MeasurementDetail = {
      id: 1, sourceId: 1, sessionId: "test", recordSeq: 1, totalCount: 1,
      productCode: "N/A", skuOrigin: "MISSING", clubNumber: 1,
      measuredAt: "2026-11-20T01:00:00Z", createdAt: "2026-11-20T01:00:00Z",
      overallResult: "OK", buttResult: "OK", tipResult: "OK", buttLoad: "2.5", tipLoad: "0.2", stations: [],
    };
    expect(renderToStaticMarkup(<MeasurementDetails data={data} />)).toContain("N/A");
    const html = renderToStaticMarkup(<MemoryRouter><RecentMeasurements records={[{ ...data, decisionPoints: [] }]}
      loading={false} error="" onRefresh={() => {}} page={1} total={1} onPageChange={() => {}} /></MemoryRouter>);
    expect(html).toContain("#1");
    expect(html).not.toContain("<input");
  });
  it("renders server totals and station results without a manual SKU editor", () => {
    const statistics: MeasurementStatistics = {
      totalMeasurements: 5, totalOk: 3, totalNg: 2, passRate: 60, tipNg: 1, buttNg: 2,
      daily: [], overallDistribution: [],
      stationResults: [{ station: "BUTT", ok: 3, ng: 2 }, { station: "TIP", ok: 4, ng: 1 }],
    };
    const html = renderToStaticMarkup(<ProductionSummary statistics={statistics} loading={false} />);
    expect(html).toContain("Toàn bộ mã sản phẩm");
    expect(html).toContain(">5<");
    expect(html).toContain(">3<");
    expect(html).toContain(">2<");
    expect(html).toContain("60% tổng số gậy");
    expect(html).toContain("BUTT"); expect(html).toContain("TIP");
    expect(html).not.toContain("SKU đang chạy");
    expect(html).not.toContain("<input");
  });
});
