import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import MeasurementDetails, { ResultTag } from "./MeasurementDetails";
import { RecentMeasurements } from "../../pages/AddMeasurement/MonitorPanels";
import type { MeasurementDetail } from "../../services/measurementService";

const zero: MeasurementDetail = {
  id: 7, sourceId: 1, sessionId: "manual-session", publicationCount: 1,
  recordSeq: 0, totalCount: 1, productCode: "QC-MANUAL", skuOrigin: "QC",
  clubNumber: 0, measuredAt: "2026-11-18T00:00:00Z", createdAt: "2026-11-18T00:00:00Z",
  overallResult: "UNMEASURED", tipResult: "UNMEASURED", buttResult: "UNMEASURED",
  tipLoad: "0", buttLoad: "0", qualityError: "Version/Status/results incomplete",
  receipt: { inboxId: 9, sourceKey: "manual-only", publicationSession: "manual-session", publicationCount: 1, wordOrder: "LOW" },
  stations: ["TIP", "BUTT"].map((stationType, i) => ({
    id: i + 1, measurementId: 7, stationType: stationType as "TIP" | "BUTT",
    result: "UNMEASURED", pointCount: 0, ngPoint: 0, load: "0",
    createdAt: "2026-11-18T00:00:00Z", points: [],
  })),
};

describe("manual publications in the existing Measurement UI", () => {
  it("renders unknown detail, zero StickNo/load, receipt provenance and no invented attempts or RAW table", () => {
    const html = renderToStaticMarkup(<MeasurementDetails data={zero} />);
    expect(html).toContain("QC-MANUAL");
    expect(html).toContain("Chưa xác định chất lượng");
    expect(html).toContain("không tính vào KPI sản xuất");
    expect(html).toContain("manual-session");
    expect(html).toContain("manual-only");
    expect(html).toContain("Inbox liên kết");
    expect(html).toContain("Khối lượng đè (g)");
    expect(html).not.toContain("Raw PLC đã nhận");
    expect(html).not.toContain("measurement-attempt-table");
    expect(renderToStaticMarkup(<ResultTag result="UNMEASURED" />)).toContain("Chưa đo");
  });

  it("keeps an unknown publication in the monitor's existing recent list with club #0", () => {
    const html = renderToStaticMarkup(<MemoryRouter><RecentMeasurements
      records={[{ ...zero, decisionPoints: [] }]} loading={false} error=""
      onRefresh={() => {}} page={1} total={1} onPageChange={() => {}} />
    </MemoryRouter>);
    expect(html).toContain("#0");
    expect(html).toContain("Chưa đo");
    expect(html).not.toContain("Raw PLC đã nhận");
  });
});
