import { describe, expect, it } from "vitest";
import { singleFlight, stableSnapshot } from "./stableSnapshot";

describe("IPC snapshot subscriptions", () => {
  it("keeps unchanged references but publishes warnings, disconnects and commits", () => {
    const old = { connection: "CONNECTED", lastSavedId: 1, warnings: ["RECONCILIATION_REQUIRED"], snapshot: { heartbeat: 1 } };
    expect(stableSnapshot(old, structuredClone(old))).toBe(old);
    const heartbeat = stableSnapshot(old, { ...old, snapshot: { heartbeat: 2 } });
    expect(heartbeat).not.toBe(old);
    expect(heartbeat.warnings).toBe(old.warnings);
    for (const next of [{ ...old, connection: "DISCONNECTED" }, { ...old, lastSavedId: 2 }, { ...old, warnings: [] }]) {
      expect(stableSnapshot(old, next)).toEqual(next);
      expect(stableSnapshot(old, next)).not.toBe(old);
    }
    expect(stableSnapshot({ a: 1, b: 2 }, { a: 1 })).toEqual({ a: 1 });
  });

  it("shares concurrent polling, then permits fresh reads and recovers after failure", async () => {
    let calls = 0;
    let release!: (value: number) => void;
    const get = singleFlight(() => { calls++; return new Promise<number>(resolve => { release = resolve; }); });
    const one = get(), two = get();
    expect(one).toBe(two);
    await Promise.resolve();
    expect(calls).toBe(1);
    const fresh = get.fresh();
    release(1);
    expect(await one).toBe(1);
    await Promise.resolve(); await Promise.resolve();
    expect(calls).toBe(2);
    release(2);
    expect(await fresh).toBe(2);
    let fail = true;
    const retry = singleFlight(async () => { if (fail) throw Error("offline"); return 3; });
    await expect(retry()).rejects.toThrow("offline");
    fail = false;
    expect(await retry()).toBe(3);
  });
});
