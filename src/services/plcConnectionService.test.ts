  import { describe, expect, it, vi } from "vitest";
  import { invoke } from "@tauri-apps/api/core";
  import { getPlcConnectionView, isPlcNetworkError, plcConnectionError, savePlcConnectionOptions } from "./plcConnectionService";
  vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
  import { getMeasurementAvailabilityNotice, getPlcDisplayStatus } from "../hooks/usePlcMonitor";
  import type { As200Status } from "./as200Service";
  describe("PLC connection form", () => {
    it("shows idle, explicit attempts, real probe failure and later recovery separately", () => {
      expect(getPlcConnectionView(undefined, null).label).toBe("Chưa kết nối PLC");
      expect(getPlcConnectionView(undefined, null).feedback).toBeUndefined();
      expect(getPlcConnectionView(undefined, "test").label).toBe("Đang kiểm tra PLC");
      expect(getPlcConnectionView(undefined, "connect").label).toBe("Đang kết nối PLC");
      const report = { tcpConnected: true, modbusReadSucceeded: true, as200Valid: true, mappingWarning: null, message: "Đã kiểm tra PLC." };
      const probe = getPlcConnectionView(undefined, null, { startedAt: 1000, report });
      expect(probe.label).toBe("Kết nối PLC hợp lệ"); expect(probe.connected).toBe(false);
      expect(getPlcConnectionView(undefined, null, {startedAt: 1000, report:{...report, as200Valid:false}}).label).toBe("Kết nối thất bại");
      const status: As200Status = {
        enabled:true, address:"127.0.0.1:1503", unitId:1, sourceKey:"test", sessionId:"test", connection:"CONNECTED", state:"WAITING",
        message:"", pauseReason:null, warnings:[], plcHealth:"RUNNING", pcHeartbeatStatus:"DISABLED",ackStatus:"RECONCILIATION_REQUIRED",
        latest:null,lastSavedId:null,blockedInboxIds:[],blockedInboxCount:0,retryWaitingCount:0,lastReadAt:new Date(3000).toISOString(),
      };
      const failure={startedAt:2000,error:"Chưa phát hiện kết nối Ethernet đến mạng PLC."};
      const recovered=getPlcConnectionView(status,null,failure);
      expect(recovered.label).toBe("Đã kết nối PLC");expect(recovered.feedback).toBeUndefined();expect(recovered.color).toBe("green");
      expect(status.ackStatus).toBe("RECONCILIATION_REQUIRED"); // never reset ACK to fix a network badge
      expect(getPlcDisplayStatus(status, "").failed).toBe(true);
      expect(getPlcConnectionView({...status,lastReadAt:new Date(1000).toISOString()},null,failure).label).toBe("Kết nối thất bại");
      expect(getPlcConnectionView({...status,connection:"DISCONNECTED"},null).label).toBe("Mất kết nối PLC");
      expect(getPlcConnectionView({...status,enabled:false,state:"STOPPED",connection:"DISABLED"},null).label).toBe("Chưa kết nối PLC");
      expect(isPlcNetworkError(failure.error)).toBe(true);
      expect(isPlcNetworkError("ACK chưa rõ; giữ pending receipt")).toBe(false);
      expect(isPlcNetworkError("PostgreSQL commit thất bại")).toBe(false);
    });
    it("saves poll timing without changing deployment adapter settings", async () => {
      vi.mocked(invoke).mockResolvedValue(undefined);
      await savePlcConnectionOptions(500);
      expect(invoke).toHaveBeenLastCalledWith("save_plc_connection_options", { pollIntervalMs: 500 });
    });
    it("keeps raw diagnostics internal while preserving operator ACK warnings", () => {
      for (const diagnostic of [
        "RAW ZERO: 240 WORD [0, 0] Inbox #42",
        "Đã lưu raw 240 WORD vào Inbox #42; chưa tạo measurement.",
        "RAW_UNDECODED: D25020–D25259",
      ]) {
        const notice = plcConnectionError(diagnostic, "fallback");
        expect(notice).toContain("Chưa có kết quả đo hợp lệ");
        expect(notice).not.toMatch(/raw|WORD|Inbox|42|D25020/i);
      }
      const warning = plcConnectionError("ACK chưa rõ, TotalCount=1 RecordSeq=0", "fallback");
      expect(warning).toContain("Không tự gửi lại ACK");
      expect(warning).not.toMatch(/TotalCount|RecordSeq/);
    });
    it("shares Worker labels without treating TCP or heartbeat as ACK confirmation", () => {
      const status: As200Status = {
        enabled: true, address: "127.0.0.1:1503", unitId: 1, sourceKey: "test", sessionId: "test-session",
        connection: "CONNECTED", state: "WAITING", message: "", pauseReason: null, warnings: [],
        plcHealth: "RUNNING", pcHeartbeatStatus: "DISABLED", ackStatus: "WAITING",
        latest: null, lastSavedId: null, blockedInboxIds: [],
        blockedInboxCount: 0, retryWaitingCount: 0,
      };
      expect(getPlcDisplayStatus(status, "").collectionLabel).toBe("Đang thu dữ liệu");
      expect(status.ackStatus).toBe("WAITING");
      const undecoded = { ...status, state: "RAW_UNDECODED", ackStatus: "ACKNOWLEDGED" as const };
      const notice = getMeasurementAvailabilityNotice(undecoded);
      expect(notice).toContain("Đã lưu dữ liệu");
      expect(notice).toContain("chưa đủ thông tin");
      expect(notice).toContain("số đo bằng 0 vẫn được chấp nhận");
      expect(notice).not.toMatch(/RAW|240|WORD|Inbox|RecordSeq|TotalCount/);
      expect(getMeasurementAvailabilityNotice({ ...status, state: "PROCESSED" })).toBeUndefined();
      expect(getMeasurementAvailabilityNotice(undefined)).toBeUndefined();
      for (const ackStatus of ["FAILED", "RECONCILIATION_REQUIRED"] as const) {
        expect(getPlcDisplayStatus({ ...status, ackStatus }, "").collectionColor).toBe("red");
        expect(getPlcDisplayStatus({ ...status, ackStatus }, "").collectionLabel).toBe("Có lỗi");
      }
      expect(getPlcDisplayStatus({ ...status, plcHealth: "INVALID_DATA" }, "").verified).toBe(false);
      expect(getPlcDisplayStatus({ ...status, plcHealth: "STALE" }, "").failed).toBe(true);
      expect(getPlcDisplayStatus({ ...status, pauseReason: "RESET" }, "").collectionLabel).toBe("Có lỗi");
      expect(getPlcDisplayStatus(status, "Không đọc được trạng thái").verified).toBe(false);
      expect(getPlcDisplayStatus(undefined, "").collectionLabel).toBe("Chưa xác định");
    });
    it("keeps technical invoke errors out of the QC message", () => {
      expect(plcConnectionError("Connection refused", "Không thể kết nối với PLC.")).toBe("Không thể kết nối với PLC.");
      const reset = "Bộ đếm máy đo đã giảm từ 2 xuống 0. Có dấu hiệu reset; không thể tiếp tục phiên cũ.";
      expect(plcConnectionError(reset, "fallback")).toBe(reset);
      expect(plcConnectionError("Cổng kết nối phải từ 1 đến 65535.", "fallback")).toBe("Cổng kết nối phải từ 1 đến 65535.");
    });
    it("preserves protective Vietnamese warnings and translates technical protective errors", () => {
      for (const warning of [
        "ACK chưa đủ bằng chứng hoàn tất; không gửi lại tín hiệu.",
        "Không lưu được Journal; dữ liệu có nguy cơ mất.",
        "Session không khớp; không tự tạo phiên mới.",
      ]) expect(plcConnectionError(warning, "fallback")).toBe(warning);
      for (const [error, expected] of [
        ["ACK checkpoint mismatch", "không tự gửi lại ACK"],
        ["ACK_CHECKPOINT_MISMATCH", "không tự gửi lại ACK"],
        ["Journal write failed", "Dữ liệu có nguy cơ mất"],
        ["SESSION_UNCONFIRMED", "không tự đổi phiên"],
        ["counter decreased after reset", "sau reset"],
      ]) {
        const message = plcConnectionError(error, "fallback");
        expect(message).toContain(expected);
        expect(message).not.toBe(error);
      }
      for (const error of ["Connection refused", "request timed out", "internal error", new Error("Connection refused"), null]) {
        expect(plcConnectionError(error, "Không thể kết nối với PLC.")).toBe("Không thể kết nối với PLC.");
      }
    });
  });
