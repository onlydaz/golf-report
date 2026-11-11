import {
  BellOutlined,
  DashboardOutlined,
  FileTextOutlined,
  HistoryOutlined,
  PlusCircleOutlined,
  SettingOutlined,
} from "@ant-design/icons";

import { Alert, Avatar, Button, Layout, Menu } from "antd";
import { useEffect, useState } from "react";
import { useLocation, useNavigate, Outlet } from "react-router-dom";
import type { MenuProps } from "antd";

import {
  getDatabaseStatus,
  retryDatabaseConnection,
  type DatabaseStatus,
} from "../services/measurementService";

const { Sider, Header, Content } = Layout;

const menuItems: MenuProps["items"] = [
  { key: "/dashboard", icon: <DashboardOutlined />, label: "Tổng quan" },
  { key: "/measurements/add", icon: <PlusCircleOutlined />, label: "Ghi nhận kết quả đo" },
  { key: "/measurements", icon: <HistoryOutlined />, label: "Lịch sử đo" },
  { key: "/reports", icon: <FileTextOutlined />, label: "Báo cáo" },
];

function MainLayout() {
  const navigate = useNavigate();
  const location = useLocation();

  const [databaseStatus, setDatabaseStatus] = useState<DatabaseStatus>({
    state: "CONNECTING",
    message: "Đang kết nối cơ sở dữ liệu...",
  });

  const [retrying, setRetrying] = useState(false);

  const selectedKey =
    location.pathname.startsWith("/measurements/") &&
    location.pathname !== "/measurements/add"
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
      <Sider width={246} className="main-sider">
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

        <div className="side-section">
          HỆ THỐNG
        </div>

        <Menu
          theme="dark"
          mode="inline"
          selectedKeys={[selectedKey]}
          onClick={({ key }) => navigate(key)}
          items={[
            {
              key: "/settings/connection",
              icon: <SettingOutlined />,
              label: "Cài đặt kết nối",
            },
          ]}
        />
      </Sider>

      <Layout>
        <Header className="main-header">
          <div className="header-context">
            Quản lý và báo cáo kết quả đo
          </div>

          <div className="header-user">

            {!databaseConnected &&
              databaseStatus.state !== "CONNECTING" && (
                <Button
                  size="small"
                  loading={retrying}
                  onClick={retryConnection}
                >
                  Thử kết nối lại
                </Button>
              )}

            <BellOutlined
              style={{ color: "#627d98", fontSize: 17 }}
            />

            <Avatar
              size={34}
              style={{
                background: "#d9eaf7",
                color: "#1769aa",
                fontWeight: 700,
              }}
            >
              QA
            </Avatar>

            <div>
              <div className="header-user-name">
                Nhân viên QC
              </div>
              <div className="header-user-role">
                Kiểm tra chất lượng
              </div>
            </div>
          </div>
        </Header>

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
              style={{ marginBottom: 20 }}
            />
          )}

          <Outlet />
        </Content>
      </Layout>
    </Layout>
  );
}

export default MainLayout;
