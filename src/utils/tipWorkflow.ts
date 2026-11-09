import type { TipPositionMeasurement } from "../types/measurement";
import { MAX_RETRY_COUNT } from "./measurementValidation";

export const createInitialTips = (): TipPositionMeasurement[] =>
  [1, 2, 3, 4].map((positionNumber) => ({
    positionNumber, finalDistance: null, retryCount: 0, result: "INCOMPLETE",
  }));

export function recordTipAttempt(tips: TipPositionMeasurement[], index: number, result: "OK" | "NG"): TipPositionMeasurement[] {
  const tip = tips[index];
  if (index !== tips.findIndex((item) => item.result !== "OK") || !tip || tip.result !== "INCOMPLETE" ||
      tip.finalDistance === null || !Number.isFinite(tip.finalDistance) || tip.finalDistance < 0) return tips;
  // Initial measurement is attempt 0. Count a retry only when its result is recorded.
  const attempt = tip.pendingRetryCount ?? 0;
  return tips.map((item, i) => {
    if (i === index) {
      if (result === "NG" && attempt < MAX_RETRY_COUNT) {
        return { ...item, finalDistance: null, retryCount: attempt, pendingRetryCount: attempt + 1 };
      }
      return { ...item, result, retryCount: attempt, pendingRetryCount: undefined };
    }
    if (i > index && result === "NG" && attempt === MAX_RETRY_COUNT) {
      return { positionNumber: item.positionNumber, finalDistance: null, retryCount: null, result: "SKIPPED" };
    }
    return item;
  });
}
