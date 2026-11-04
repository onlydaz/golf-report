import type { Measurement } from "../types/measurement";

export const measurements: Measurement[] = [
  { id: "MR-260104-001", measuredAt: "2026-01-04 09:42", productCode: "GFL-STEEL-07", productName: "Steel Shaft 7I", operator: "Nguyen Van An", station: "Line A / S01", status: "OK", buttDiameter: 15.10, buttRoundness: 0.018, tips: [
    { position: "P1", angle: 0.12, diameter: 9.02, runout: 0.022, status: "OK" }, { position: "P2", angle: 0.09, diameter: 9.01, runout: 0.019, status: "OK" }, { position: "P3", angle: 0.11, diameter: 9.02, runout: 0.021, status: "OK" }, { position: "P4", angle: 0.10, diameter: 9.01, runout: 0.020, status: "OK" },
  ] },
  { id: "MR-260104-002", measuredAt: "2026-01-04 09:31", productCode: "GFL-CARBON-01", productName: "Carbon Shaft Driver", operator: "Tran Minh Duc", station: "Line A / S02", status: "OK", buttDiameter: 15.32, buttRoundness: 0.023, tips: [
    { position: "P1", angle: 0.08, diameter: 8.95, runout: 0.018, status: "OK" }, { position: "P2", angle: 0.10, diameter: 8.96, runout: 0.021, status: "OK" }, { position: "P3", angle: 0.09, diameter: 8.95, runout: 0.019, status: "OK" }, { position: "P4", angle: 0.11, diameter: 8.96, runout: 0.022, status: "OK" },
  ] },
  { id: "MR-260104-003", measuredAt: "2026-01-04 09:05", productCode: "GFL-STEEL-09", productName: "Steel Shaft 9I", operator: "Le Hoang Nam", station: "Line B / S01", status: "NG", buttDiameter: 15.18, buttRoundness: 0.061, tips: [
    { position: "P1", angle: 0.12, diameter: 9.03, runout: 0.028, status: "OK" }, { position: "P2", angle: 0.15, diameter: 9.04, runout: 0.031, status: "OK" }, { position: "P3", angle: 0.19, diameter: 9.08, runout: 0.061, status: "NG" }, { position: "P4", angle: 0.14, diameter: 9.04, runout: 0.030, status: "OK" },
  ] },
  { id: "MR-260104-004", measuredAt: "2026-01-04 08:48", productCode: "GFL-CARBON-01", productName: "Carbon Shaft Driver", operator: "Nguyen Van An", station: "Line A / S01", status: "OK", buttDiameter: 15.30, buttRoundness: 0.020, tips: [
    { position: "P1", angle: 0.07, diameter: 8.94, runout: 0.017, status: "OK" }, { position: "P2", angle: 0.09, diameter: 8.95, runout: 0.018, status: "OK" }, { position: "P3", angle: 0.08, diameter: 8.95, runout: 0.018, status: "OK" }, { position: "P4", angle: 0.10, diameter: 8.96, runout: 0.020, status: "OK" },
  ] },
  { id: "MR-260104-005", measuredAt: "2026-01-04 08:26", productCode: "GFL-STEEL-07", productName: "Steel Shaft 7I", operator: "Pham Gia Bao", station: "Line B / S02", status: "OK", buttDiameter: 15.12, buttRoundness: 0.019, tips: [
    { position: "P1", angle: 0.11, diameter: 9.01, runout: 0.020, status: "OK" }, { position: "P2", angle: 0.10, diameter: 9.02, runout: 0.021, status: "OK" }, { position: "P3", angle: 0.12, diameter: 9.02, runout: 0.022, status: "OK" }, { position: "P4", angle: 0.09, diameter: 9.01, runout: 0.019, status: "OK" },
  ] },
  { id: "MR-260104-006", measuredAt: "2026-01-04 08:02", productCode: "GFL-STEEL-09", productName: "Steel Shaft 9I", operator: "Tran Minh Duc", station: "Line B / S01", status: "NG", buttDiameter: 15.08, buttRoundness: 0.054, tips: [
    { position: "P1", angle: 0.13, diameter: 9.02, runout: 0.024, status: "OK" }, { position: "P2", angle: 0.13, diameter: 9.02, runout: 0.025, status: "OK" }, { position: "P3", angle: 0.13, diameter: 9.03, runout: 0.026, status: "OK" }, { position: "P4", angle: 0.22, diameter: 9.09, runout: 0.068, status: "NG" },
  ] },
];

export const chartData = [
  { day: "Dec 29", ok: 38, ng: 3 }, { day: "Dec 30", ok: 45, ng: 2 }, { day: "Dec 31", ok: 42, ng: 4 },
  { day: "Jan 01", ok: 29, ng: 1 }, { day: "Jan 02", ok: 49, ng: 3 }, { day: "Jan 03", ok: 52, ng: 2 }, { day: "Jan 04", ok: 46, ng: 3 },
];
