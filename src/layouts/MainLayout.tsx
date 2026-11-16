import { Alert, Button, Layout, Menu } from "antd";
import { useCallback, useEffect, useRef, useState } from "react";
import { useLocation, useNavigate, Outlet } from "react-router-dom";
import type { MenuProps } from "antd";
import { getAs200WorkerStatus, type As200Status } from "../services/as200Service";
import { getPlcConnectionSettings, plcConnectionError, type PlcConnectionSettings } from "../services/plcConnectionService";

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

function MainLayout() {
  const navigate = useNavigate();
  const location = useLocation();

  const [databaseStatus, setDatabaseStatus] = useState<DatabaseStatus>({
    state: "CONNECTING",
    message: "Đang kết nối cơ sở dữ liệu...",
  });

  const [retrying, setRetrying] = useState(false);
  const [status, setStatus] = useState<As200Status>();
  const [settings, setSettings] = useState<PlcConnectionSettings>();
  const [workerError, setWorkerError] = useState("");
  const mounted = useRef(false);
  const workerRequest = useRef(0);

  const refreshWorker = useCallback(async () => {
    const request = ++workerRequest.current;
    try {
      const next = await getAs200WorkerStatus();
      if (mounted.current && request === workerRequest.current) {
        setStatus(next);
      }
      const connection = await getPlcConnectionSettings();
      if (mounted.current && request === workerRequest.current) {
        setSettings(connection); setWorkerError("");
      }
      return next;
    } catch (reason) {
      if (mounted.current && request === workerRequest.current) {
        setWorkerError(plcConnectionError(reason, "Không đọc được trạng thái hệ thống. Vui lòng thử lại."));
      }
      throw reason;
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try { await refreshWorker(); } catch { /* The shared error is rendered by consumers. */ }
      if (active) timer = setTimeout(() => void poll(), 500);
    };
    void poll();
    return () => {
      active = false; mounted.current = false; ++workerRequest.current; clearTimeout(timer);
    };
  }, [refreshWorker]);

  const selectedKey =
    location.pathname.startsWith("/measurements/")
      ? "/measurements"
      : location.pathname;

  useEffect(() => {
    let active = true;
    let timer: number | undefined;

    const refreshStatus = async () => {
      try {
        const status = await getDatabaseStatus();

        if (active) {
          setDatabaseStatus(status);
        }
      } catch {
        if (active) {
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
  }, []);

  const retryConnection = async () => {
    setRetrying(true);

    try {
      setDatabaseStatus(await retryDatabaseConnection());
    } catch {
      setDatabaseStatus({
        state: "CONNECTION_FAILED",
        message: "Không thể thử kết nối cơ sở dữ liệu.",
      });
    } finally {
      setRetrying(false);
    }
  };

  const databaseConnected = databaseStatus.state === "CONNECTED";

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
          {!databaseConnected && (
            <Alert
              type={
                databaseStatus.state === "CONNECTING"
                  ? "info"
                  : "error"
              }
              showIcon
              message={databaseStatus.message}
              action={databaseStatus.state !== "CONNECTING" ? <Button size="small" loading={retrying} onClick={() => void retryConnection()}>Thử kết nối lại</Button> : undefined}
              style={{ marginBottom: 20 }}
            />
          )}

          <Outlet context={{ status, settings, error: workerError, databaseConnected, refresh: refreshWorker }} />
        </Content>
      </Layout>
    </Layout>
  );
}

export default MainLayout;
