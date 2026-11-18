import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { MemoryRouter } from "react-router-dom";
import ConnectionSettings from "./ConnectionSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../../hooks/usePlcMonitor", async importOriginal => ({ ...await importOriginal<typeof import("../../hooks/usePlcMonitor")>(), usePlcMonitor: () => ({
  status: undefined, settings: undefined, error: "", refresh: vi.fn(),
}) }));

describe("QC PLC connection page", () => {
  it("keeps operator controls and status without network selection or idle Ethernet alerts", () => {
    vi.mocked(invoke).mockClear();
    const html = renderToStaticMarkup(<MemoryRouter initialEntries={["/connection-settings"]}><ConnectionSettings /></MemoryRouter>);
    for (const label of ["4 · Kết nối PLC", "Thiết lập kỹ thuật", "Kiểm tra kết nối", "Kết nối", "Trạng thái kết nối", "Nhật ký kết nối PLC", "Chưa kết nối PLC", "Heartbeat PLC"]) {
      expect(html).toContain(label);
    }
    expect(html).not.toContain("Chỉnh thông số trong as200_config.toml");
    for (const removed of ["Kết nối mạng PLC", "Automatic", "Manual", "Cập nhật card mạng", "Khắc phục đường mạng", "Chưa phát hiện kết nối Ethernet", "Lưu cài đặt", "ant-input-number"]) {
      expect(html).not.toContain(removed);
    }
    expect(invoke).not.toHaveBeenCalled();
  });
});
