import { ConfigProvider, Spin } from "antd";
import viVN from "antd/locale/vi_VN";
import { lazy, Suspense } from "react";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import MainLayout from "./layouts/MainLayout";
import AddMeasurement from "./pages/AddMeasurement/AddMeasurement";
import Dashboard from "./pages/Dashboard/Dashboard";
import ConnectionSettings from "./pages/ConnectionSettings/ConnectionSettings";
import "./App.css";

const MeasurementDetail = lazy(() => import("./pages/MeasurementDetail/MeasurementDetail"));
const MeasurementHistory = lazy(() => import("./pages/MeasurementHistory/MeasurementHistory"));
const Reports = lazy(() => import("./pages/Reports/Reports"));

function App() {
  return (
    <ConfigProvider locale={viVN}
      theme={{
        token: {
          colorPrimary: "#1769aa",
          colorInfo: "#1769aa",
          colorSuccess: "#1d9b6c",
          colorWarning: "#db9b2b",
          colorError: "#d95757",
          borderRadius: 10,
          fontFamily: "Inter, Segoe UI, sans-serif",
        },
        components: {
          Layout: { siderBg: "#102a43", headerBg: "#ffffff" },
          Menu: { darkItemBg: "#102a43", darkItemSelectedBg: "#1b527e" },
          Table: { headerBg: "#f5f8fb", headerColor: "#486581" },
        },
      }}
    >
      <BrowserRouter>
        <Routes>
          <Route element={<MainLayout />}>
            <Route index element={<Navigate to="/dashboard" replace />} />
            <Route path="/dashboard" element={<Dashboard />} />
            <Route path="/measurements/add" element={<AddMeasurement />} />
            <Route path="/measurements" element={
              <Suspense fallback={<Spin size="large" />}>
                <MeasurementHistory />
              </Suspense>
            } />
            <Route path="/measurements/:measurementId" element={
              <Suspense fallback={<Spin size="large" />}>
                <MeasurementDetail />
              </Suspense>
            } />
            <Route path="/reports" element={
              <Suspense fallback={<Spin size="large" />}>
                <Reports />
              </Suspense>
            } />
            <Route path="/settings/connection" element={<ConnectionSettings />} />
          </Route>
        </Routes>
      </BrowserRouter>
    </ConfigProvider>
  );
}

export default App;
