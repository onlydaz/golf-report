import { describe, expect, it } from "vitest";
import { createInitialTips, recordTipAttempt } from "./tipWorkflow";
import { calculateTipResult, validateTipPosition } from "./measurementValidation";

const record = (tips: ReturnType<typeof createInitialTips>, result: "OK" | "NG", index = 0, distance = 12) =>
  recordTipAttempt(tips.map((tip, i) => i === index ? { ...tip, finalDistance: distance } : tip), index, result);

describe("Tip sequential workflow", () => {
  it("does not count the initial NG or entering the next distance as a retry", () => {
    const initial = createInitialTips();
    expect(initial[0].retryCount).toBe(0);
    const firstNg = record(initial, "NG", 0, 15);
    expect(firstNg[0]).toMatchObject({ retryCount: 0, pendingRetryCount: 1, result: "INCOMPLETE" });
    const enteringRetry = firstNg.map((tip, i) => i === 0 ? { ...tip, finalDistance: 11 } : tip);
    expect(enteringRetry[0].retryCount).toBe(0);
    const retryOk = recordTipAttempt(enteringRetry, 0, "OK");
    expect(retryOk[0]).toMatchObject({ retryCount: 1, finalDistance: 11, result: "OK" });
    expect(retryOk[1]).toMatchObject({ retryCount: 0, result: "INCOMPLETE" });
  });
  it("keeps initial NG and retries 1 through 3 incomplete, then skips only after retry 4 NG", () => {
    let tips = createInitialTips();
    for (let attempt = 0; attempt < 4; attempt++) {
      tips = record(tips, "NG");
      expect(tips[0]).toMatchObject({ result: "INCOMPLETE", retryCount: attempt, pendingRetryCount: attempt + 1, finalDistance: null });
      expect(tips.slice(1).every(tip => tip.result === "INCOMPLETE")).toBe(true);
      expect(calculateTipResult(tips)).toBe("INCOMPLETE");
    }
    tips = record(tips, "NG", 0, 8);
    expect(tips[0]).toMatchObject({ result: "NG", retryCount: 4, finalDistance: 8 });
    expect(tips.slice(1).every(tip => tip.result === "SKIPPED" && tip.finalDistance === null && tip.retryCount === null)).toBe(true);
    expect(calculateTipResult(tips)).toBe("NG");
  });
  it.each([0, 1, 2, 3, 4])("accepts OK after %i retries and advances", (retries) => {
    let tips = createInitialTips();
    for (let i = 0; i < retries; i++) tips = record(tips, "NG");
    tips = record(tips, "OK", 0, 9);
    expect(tips[0]).toMatchObject({ result: "OK", retryCount: retries, finalDistance: 9 });
    for (let i = 1; i < 4; i++) tips = record(tips, "OK", i);
    expect(calculateTipResult(tips)).toBe("OK");
  });
  it("blocks missing or invalid distances and future or finished positions", () => {
    const tips = createInitialTips();
    expect(recordTipAttempt(tips, 0, "OK")).toBe(tips);
    for (const distance of [-1, NaN, Infinity]) expect(record(tips, "OK", 0, distance)[0].result).toBe("INCOMPLETE");
    expect(record(tips, "OK", 1)[1].result).toBe("INCOMPLETE");
    const finished = record(tips, "OK");
    expect(recordTipAttempt(finished, 0, "NG")).toBe(finished);
  });
  it("rejects skipped positions without preceding NG and incomplete positions with values", () => {
    const tips = createInitialTips();
    tips[0] = { ...tips[0], result: "SKIPPED", retryCount: null };
    expect(validateTipPosition(tips[0], 0, tips).length).toBeGreaterThan(0);
    tips[0] = { ...createInitialTips()[0], finalDistance: 10 };
    expect(validateTipPosition(tips[0], 0, tips).length).toBeGreaterThan(0);
  });
});
