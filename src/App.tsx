import { ConfigProvider } from "antd";
import viVN from "antd/locale/vi_VN";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import MainLayout from "./layouts/MainLayout";
import AddMeasurement from "./pages/AddMeasurement/AddMeasurement";
import Dashboard from "./pages/Dashboard/Dashboard";
import MeasurementDetail from "./pages/MeasurementDetail/MeasurementDetail";
import MeasurementHistory from "./pages/MeasurementHistory/MeasurementHistory";
import Reports from "./pages/Reports/Reports";
import "./App.css";

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
            <Route path="/measurements" element={<MeasurementHistory />} />
            <Route path="/measurements/:measurementId" element={<MeasurementDetail />} />
            <Route path="/reports" element={<Reports />} />
          </Route>
        </Routes>
      </BrowserRouter>
    </ConfigProvider>
  );
}

export default App;
