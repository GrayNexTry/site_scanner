import { NavLink, Route, Routes } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { api } from "./api";
import ScansPage from "./pages/ScansPage";
import NewScanPage from "./pages/NewScanPage";
import ScanPage from "./pages/ScanPage";
import BusinessPage from "./pages/BusinessPage";
import CatalogPage from "./pages/CatalogPage";
import SettingsPage from "./pages/SettingsPage";
import LeadsPage from "./pages/LeadsPage";
import ReportPage from "./pages/ReportPage";

function Logo() {
  return (
    <svg width="22" height="22" viewBox="0 0 32 32" aria-hidden="true">
      <circle cx="14" cy="14" r="9" fill="none" stroke="var(--hot)" strokeWidth="3" />
      <path d="M21 21l7 7" stroke="var(--hot)" strokeWidth="3" strokeLinecap="round" />
    </svg>
  );
}

export default function App() {
  return (
    <Routes>
      <Route path="/report/:id" element={<ReportPage />} />
      <Route path="*" element={<Shell />} />
    </Routes>
  );
}

function Shell() {
  const net = useQuery({ queryKey: ["net"], queryFn: api.net, staleTime: 5 * 60_000 });
  const meta = useQuery({ queryKey: ["meta"], queryFn: api.meta, staleTime: 60_000 });
  return (
    <div className="app">
      <aside className="side">
        <div className="brand">
          <Logo />
          <span>Сканер карт</span>
        </div>
        <nav className="nav">
          <NavLink to="/" end>Сканы</NavLink>
          <NavLink to="/new">Новый скан</NavLink>
          <NavLink to="/leads">Лиды</NavLink>
          <NavLink to="/catalog">Метрики</NavLink>
          <NavLink to="/settings">Настройки</NavLink>
        </nav>
        <div className="side-foot">
          {net.data?.country && (
            <span title={`IP ${net.data.ip ?? ""}`}>
              Скан идёт из: <b className={net.data.country === "RU" ? "" : "st-warn"}>{net.data.country}</b>
            </span>
          )}
          {meta.data && <span>Версия {meta.data.version} · {meta.data.catalog.metrics.length} метрик</span>}
        </div>
      </aside>
      <main className="main">
        <Routes>
          <Route path="/" element={<ScansPage />} />
          <Route path="/new" element={<NewScanPage />} />
          <Route path="/scans/:id" element={<ScanPage />} />
          <Route path="/businesses/:id" element={<BusinessPage />} />
          <Route path="/leads" element={<LeadsPage />} />
          <Route path="/catalog" element={<CatalogPage />} />
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="*" element={<div className="page"><h1>Страница не найдена</h1></div>} />
        </Routes>
      </main>
    </div>
  );
}
