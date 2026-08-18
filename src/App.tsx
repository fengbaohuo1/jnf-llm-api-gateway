import { Routes, Route, Navigate } from "react-router-dom";
import { Sidebar } from "./components/Sidebar";
import { Dashboard } from "./pages/Dashboard";
import { Usage } from "./pages/Usage";
import { Channels } from "./pages/Channels";
import { Keys } from "./pages/Keys";
import { Logs } from "./pages/Logs";
import { Settings } from "./pages/Settings";

function App() {
  return (
    <div className="flex min-h-screen bg-gray-50 text-gray-900">
      <Sidebar />
      <div className="mx-auto w-full max-w-6xl flex-1 px-8 py-8">
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/usage" element={<Usage />} />
          <Route path="/channels" element={<Channels />} />
          <Route path="/keys" element={<Keys />} />
          <Route path="/logs" element={<Logs />} />
          <Route path="/settings" element={<Settings />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </div>
    </div>
  );
}

export default App;
