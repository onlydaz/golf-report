import { Alert, Button, Layout, Menu } from "antd";
import { createContext, memo, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useLocation, useNavigate, Outlet } from "react-router-dom";
import type { MenuProps } from "antd";
import { getAs200WorkerStatus, type As200Status } from "../services/as200Service";
import { getPlcConnectionSettings, plcConnectionError, type PlcConnectionSettings } from "../services/plcConnectionService";
import { DatabaseConnectedContext, MeasurementCommitContext, PlcRefreshContext, PlcSettingsContext, PlcStatusContext } from "../hooks/usePlcMonitor";
import { singleFlight, stableSnapshot } from "../utils/stableSnapshot";

import {
  getDatabaseStatus,
  retryDatabaseConnection,
  type DatabaseStatus,
} from "../services/measurementService";

const { Sider, Content } = Layout;

const menuItems: MenuProps["items"] = [
  { key: "/monitor", icon: <span className="sidebar-number">1</span>, label: "Giám sát" },
  { key: "/measurements", icon: <span className="sidebar-number">2</span>, label: "Lịch sử" },
  { key: "/reports", icon: <span className="sidebar-number">3</span>, label: "Báo cáo" },
  { key: "/settings/connection", icon: <span className="sidebar-number">4</span>, label: "Kết nối PLC" },
];

const DatabaseBannerContext = createContext<{ status: DatabaseStatus; retrying: boolean; retry: () => Promise<void> } | null>(null);

function MonitorProvider({ children }: { children: ReactNode }) {

  const [databaseStatus, setDatabaseStatus] = useState<DatabaseStatus>({
    state: "CONNECTING",
    message: "Đang kết nối cơ sở dữ liệu...",
  });

  const [retrying, setRetrying] = useState(false);
  const [status, setStatus] = useState<As200Status>();
  const [settings, setSettings] = useState<PlcConnectionSettings>();
  const [workerError, setWorkerError] = useState("");
  const [settingsError, setSettingsError] = useState("");
  const mounted = useRef(false);
  const generation = useRef(0);
  const settingsAttempt = useRef(0);
  const settingsHealthy = useRef(false);
  const databaseRequest = useRef(0);
  const databaseRetrying = useRef(false);
  const readDatabase = useMemo(() => singleFlight(getDatabaseStatus), []);

  const readStatus = useMemo(() => singleFlight(async () => {
    const current = generation.current;
    try {
      const next = await getAs200WorkerStatus();
      if (mounted.current && current === generation.current) {
        setStatus(previous => stableSnapshot(previous, next)); setWorkerError("");
      }
      return next;
    } catch (reason) {
      if (mounted.current && current === generation.current) setWorkerError(plcConnectionError(reason, "Không đọc được trạng thái hệ thống. Vui lòng thử lại."));
      throw reason;
    }
  }), []);
  const readSettings = useMemo(() => singleFlight(async () => {
    const current = generation.current;
    settingsAttempt.current = Date.now();
    try {
      const next = await getPlcConnectionSettings();
      settingsHealthy.current = true;
      if (mounted.current && current === generation.current) {
        setSettings(previous => stableSnapshot(previous, next)); setSettingsError("");
      }
      return next;
    } catch (reason) {
      settingsHealthy.current = false;
      if (mounted.current && current === generation.current) setSettingsError(plcConnectionError(reason, "Không đọc được cấu hình PLC."));
      throw reason;
    }
  }), []);
  const refreshWorker = useCallback(async () => {
    const [next] = await Promise.all([readStatus.fresh(), readSettings.fresh()]);
    return next;
  }, [readStatus, readSettings]);

  useEffect(() => {
    mounted.current = true;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try { await readStatus(); } catch { /* The shared error is rendered by consumers. */ }
      if (active && Date.now() - settingsAttempt.current >= (settingsHealthy.current ? 30000 : 5000)) {
        try { await readSettings(); } catch { /* Keep the settings error visible. */ }
      }
      if (active) timer = setTimeout(() => void poll(), 500);
    };
    void readSettings().catch(() => undefined);
    void poll();
    return () => {
      active = false; mounted.current = false; ++generation.current; clearTimeout(timer);
    };
  }, [readStatus, readSettings]);

  useEffect(() => {
    let active = true;
    let timer: number | undefined;

    const refreshStatus = async () => {
      if (databaseRetrying.current) {
        if (active) timer = window.setTimeout(() => void refreshStatus(), 2000);
        return;
      }
      const request = ++databaseRequest.current;
      try {
        const status = await readDatabase();

        if (active && request === databaseRequest.current) {
          setDatabaseStatus(previous => stableSnapshot(previous, status));
        }
      } catch {
        if (active && request === databaseRequest.current) {
          setDatabaseStatus({
            state: "CONNECTION_FAILED",
            message: "Không thể đọc trạng thái cơ sở dữ liệu.",
          });
        }
      } finally {
        if (active) timer = window.setTimeout(() => void refreshStatus(), 2000);
      }
    };

    void refreshStatus();

    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [readDatabase]);

  const retryConnection = useCallback(async () => {
    if (databaseRetrying.current) return;
    databaseRetrying.current = true;
    const request = ++databaseRequest.current;
    setRetrying(true);

    try {
      const next = await retryDatabaseConnection();
      if (mounted.current && request === databaseRequest.current) setDatabaseStatus(previous => stableSnapshot(previous, next));
    } catch {
      if (mounted.current && request === databaseRequest.current) setDatabaseStatus({
        state: "CONNECTION_FAILED",
        message: "Không thể thử kết nối cơ sở dữ liệu.",
      });
    } finally {
      databaseRetrying.current = false;
      if (mounted.current) setRetrying(false);
    }
  }, []);

  const databaseConnected = databaseStatus.state === "CONNECTED";
  const plc = useMemo(() => ({ status, error: workerError || settingsError }), [status, workerError, settingsError]);
  const databaseBanner = useMemo(() => ({ status: databaseStatus, retrying, retry: retryConnection }), [databaseStatus, retrying, retryConnection]);

  return <DatabaseConnectedContext.Provider value={databaseConnected}>
    <DatabaseBannerContext.Provider value={databaseBanner}>
      <PlcSettingsContext.Provider value={settings}>
        <PlcRefreshContext.Provider value={refreshWorker}>
          <MeasurementCommitContext.Provider value={status?.lastSavedId ?? null}>
            <PlcStatusContext.Provider value={plc}>{children}</PlcStatusContext.Provider>
          </MeasurementCommitContext.Provider>
        </PlcRefreshContext.Provider>
      </PlcSettingsContext.Provider>
    </DatabaseBannerContext.Provider>
  </DatabaseConnectedContext.Provider>;
}

function DatabaseBanner() {
  const view = useContext(DatabaseBannerContext)!;
  if (view.status.state === "CONNECTED") return null;
  return <Alert type={view.status.state === "CONNECTING" ? "info" : "error"} showIcon message={view.status.message}
    action={view.status.state !== "CONNECTING" ? <Button size="small" loading={view.retrying} onClick={() => void view.retry()}>Thử kết nối lại</Button> : undefined}
    style={{ marginBottom: 20 }} />;
}

const LayoutShell = memo(function LayoutShell() {
  const navigate = useNavigate();
  const location = useLocation();
  const selectedKey = location.pathname.startsWith("/measurements/") ? "/measurements" : location.pathname;

  return (
    <Layout className="app-layout">
      <Sider width={232} className="main-sider">
        <div className="brand">
          <div className="brand-mark">G</div>

          <div>
            <div className="brand-title">Golf Report</div>
            <div className="brand-subtitle">Báo cáo kết quả đo</div>
          </div>
        </div>

        <div className="side-section">
          CHỨC NĂNG CHÍNH
        </div>

        <Menu
          theme="dark"
          mode="inline"
          selectedKeys={[selectedKey]}
          items={menuItems}
          onClick={({ key }) => navigate(key)}
        />

      </Sider>

      <Layout>
        <Content className="main-content">
          <DatabaseBanner />
          <Outlet />
        </Content>
      </Layout>
    </Layout>
  );
});

function MainLayout() {
  return <MonitorProvider><LayoutShell /></MonitorProvider>;
}

export default MainLayout;
