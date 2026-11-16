import type { ReactNode } from "react";
import { useLocation } from "react-router-dom";
import { getPlcDisplayStatus, usePlcMonitor } from "../hooks/usePlcMonitor";

interface PageHeaderProps {
  eyebrow: string;
  title: string;
  description: string;
  extra?: ReactNode;
}

function PageHeader({
  title,
  extra,
}: PageHeaderProps) {
  const { pathname } = useLocation();
  const { status, settings, error } = usePlcMonitor();
  const pageTitle = pathname === "/monitor" ? "1 · Giám sát"
    : pathname === "/measurements" ? "2 · Lịch sử"
    : pathname.startsWith("/measurements/") ? "2 · Chi tiết phép đo"
    : pathname === "/reports" ? "3 · Báo cáo"
    : pathname === "/settings/connection" ? "4 · Kết nối PLC" : title;
  const { verified, failed, plcLabel, collectionLabel, plcColor } = getPlcDisplayStatus(status, error);
  const plcIp = status?.address ? status.address.replace(/:\d+$/, "") : settings?.endpoint?.ip;
  return (
    <>
    <header className="page-heading">
        <h1 className="page-title">
          {pageTitle}
        </h1>
        <div className="page-status" aria-live="polite">
          <span className={`page-status-pill ${verified ? "page-status-connected" : plcColor === "red" ? "page-status-error" : "page-status-unverified"}`}>
            <i aria-hidden="true" />{verified ? `PLC ${plcIp ?? ""} · Đã kết nối` : plcLabel}
          </span>
          <span className={`page-status-pill ${failed || verified && status?.enabled ? "page-status-recording" : "page-status-unverified"}`}>
            <i aria-hidden="true" />{collectionLabel === "Đã dừng" ? "Đã dừng thu dữ liệu" : collectionLabel}
          </span>
        </div>
    </header>
    {extra && <div className="page-header-actions">{extra}</div>}
    </>
  );
}

export default PageHeader;
