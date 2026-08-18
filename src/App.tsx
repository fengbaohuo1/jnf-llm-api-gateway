import { Routes, Route, NavLink, Navigate } from "react-router-dom";
import { Home } from "./pages/Home";
import { Settings } from "./pages/Settings";

function App() {
  return (
    <div className="min-h-screen bg-gray-50 text-gray-900">
      <nav className="flex items-center gap-4 border-b border-gray-200 bg-white px-6 py-3">
        <h1 className="text-lg font-semibold">LLM Gateway</h1>
        <NavLink
          to="/"
          className={({ isActive }) =>
            `px-3 py-1 rounded-md ${isActive ? "bg-indigo-600 text-white" : "text-gray-700 hover:bg-gray-100"}`
          }
        >
          Home
        </NavLink>
        <NavLink
          to="/settings"
          className={({ isActive }) =>
            `px-3 py-1 rounded-md ${isActive ? "bg-indigo-600 text-white" : "text-gray-700 hover:bg-gray-100"}`
          }
        >
          Settings
        </NavLink>
      </nav>

      <main className="p-6">
        <Routes>
          <Route path="/" element={<Home />} />
          <Route path="/settings" element={<Settings />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
    </div>
  );
}

export default App;
