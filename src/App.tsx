import { Routes, Route, Navigate } from "react-router-dom";
import { Sidebar } from "./components/Sidebar";
import { Dashboard } from "./pages/Dashboard";
import { Usage } from "./pages/Usage";
import { Channels } from "./pages/Channels";
import { Keys } from "./pages/Keys";
import { Logs } from "./pages/Logs";
import { Security } from "./pages/Security";
import { Settings } from "./pages/Settings";

function App() {
  return (
    <div className="flex h-screen overflow-hidden bg-app text-main">
      <Sidebar />
      <div className="min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[1440px] px-4 py-4">
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/usage" element={<Usage />} />
            <Route path="/channels" element={<Channels />} />
            <Route path="/keys" element={<Keys />} />
            <Route path="/logs" element={<Logs />} />
            <Route path="/security" element={<Security />} />
            <Route path="/settings" element={<Settings />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </div>
      </div>
    </div>
  );
}

export default App;
