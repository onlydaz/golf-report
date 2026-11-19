import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { BendComparison, BendDot, BendSettingsSummary } from "./MonitorPanels";
import { measurementSettings } from "../../config/measurementSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("fixed production chart settings", () => {
  it.each(["GOLD-X1", "DEMO-AS200", undefined])("shows source defaults without config UI or IPC for %s", sku => {
    vi.mocked(invoke).mockClear();
    const html = renderToStaticMarkup(<BendComparison records={[]} productCode={sku} loading={false} error="" />);
    for (const text of ["0,5 kg", "10,00 mm", "50 - 200 mm", "150 · 300 · 450 · 600 mm"]) expect(html.replace(/<[^>]*>/g, "")).toContain(text);
    expect(html).not.toContain("Lực đè cài đặt");
    expect(html).not.toContain("Thông số mặc định từ source");
    expect(html).not.toContain("Giới hạn hiện tại chỉ để đối chiếu trực quan");
    expect(html).not.toContain("Cấu hình thông số đo");
    expect(html).not.toContain("Chưa cấu hình");
    expect(invoke).not.toHaveBeenCalled();
  });
  it("shows the documented TIP 80 mm position and station-specific defaults", () => {
    const html = renderToStaticMarkup(<BendSettingsSummary station="TIP" settings={measurementSettings.TIP} />);
    for (const text of ["0,2 kg", "8,00 mm", "50 - 200 mm", "80 mm"]) expect(html.replace(/<[^>]*>/g, "")).toContain(text);
    expect(html).not.toContain("●");
    for (const removed of ["Khoảng cách đẩy cài đặt", "Vị trí điểm đo cài đặt", "Dưới", "Trên"]) expect(html).not.toContain(removed);
  });
  it("preserves zero and small decimals when technicians edit source settings", () => {
    const html = renderToStaticMarkup(<BendSettingsSummary station="TIP" settings={{ ...measurementSettings.TIP, loadG: 0, bendLowerMm: 0, bendUpperMm: 0.001 }} />);
    expect(html).toContain("0 kg"); expect(html).toContain("0 - 0,001 mm");
  });
  it("uses red rings only outside the configured range, keeping NULL and boundary values distinct", () => {
    const dot = (value: number | null) => renderToStaticMarkup(<svg><BendDot cx={10} cy={20} value={value} color="blue" settings={measurementSettings.BUTT} /></svg>);
    for (const value of [49, 201]) {
      expect(dot(value)).toContain('stroke="#d92d20"'); expect(dot(value)).toContain('fill="white"');
    }
    for (const value of [50, 125, 200]) expect(dot(value)).toContain('stroke="blue"');
    expect(dot(null)).not.toContain("circle"); expect(dot(NaN)).not.toContain("circle");
  });
});
