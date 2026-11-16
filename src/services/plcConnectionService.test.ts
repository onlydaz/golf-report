  import { describe, expect, it } from "vitest";
  import { isPlcIp, plcConnectionError } from "./plcConnectionService";
  describe("PLC connection form", () => {
    it("accepts IP literals and rejects hosts, malformed IPs or embedded ports", () => {
      for (const ip of ["127.0.0.1", "192.168.1.10", "::1", "2001:db8::1"]) expect(isPlcIp(ip)).toBe(true);
      for (const ip of ["localhost", "127.0.0.256", "1.2.3", "1.2.3.4:502", "01.2.3.4", "::xyz", "", "[::1]", " 127.0.0.1"]) expect(isPlcIp(ip)).toBe(false);
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
