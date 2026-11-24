// Мелкие общие компоненты.
import { useEffect, useState, type ReactNode } from "react";
import type { Lead, Place, Scan, Status } from "./api";
import { LEAD_HINT, LEAD_LABEL, PROVIDER_LABEL, SCAN_STATUS, scoreTone, STATUS_LABEL } from "./format";

export function LeadBadge({ lead, pending }: { lead: Lead | null; pending?: string }) {
  if (!lead) return <span className="badge lead-pending">{pending ?? "Ждёт"}</span>;
  return (
    <span className={`badge lead-${lead}`} title={LEAD_HINT[lead]}>
      {LEAD_LABEL[lead]}
    </span>
  );
}

export function StatusDot({ status }: { status: Status }) {
  return <span className={`dot st-${status}`} title={STATUS_LABEL[status]} aria-label={STATUS_LABEL[status]} />;
}

export function ScoreBar({ value }: { value: number | null | undefined }) {
  const tone = scoreTone(value);
  return (
    <div className="score" title={value == null ? "Балла нет" : `${value} из 100`}>
      <b className={value == null ? "faint" : ""}>{value == null ? "—" : Math.round(value)}</b>
      <div className={`bar ${tone}`}>
        <span style={{ width: `${Math.max(0, Math.min(100, value ?? 0))}%` }} />
      </div>
    </div>
  );
}

export function Progress({ done, total, active }: { done: number; total: number; active: boolean }) {
  const indeterminate = active && total === 0;
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;
  return (
    <div className={`progress${indeterminate ? " indeterminate" : ""}`} role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
      <span style={indeterminate ? undefined : { width: `${pct}%` }} />
    </div>
  );
}

export function ScanStatus({ scan }: { scan: Scan }) {
  const tone =
    scan.status === "done" ? "st-pass" : scan.status === "failed" ? "st-fail" : scan.status === "running" || scan.status === "queued" ? "" : "st-warn";
  let label = SCAN_STATUS[scan.status];
  if (scan.status === "running") label = scan.stage === "places" ? "Ищу места" : `Проверяю сайты ${scan.done}/${scan.total}`;
  return <span className={`small ${tone}`}>{label}</span>;
}

export function Empty({ title, children, action }: { title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty">
      <h2>{title}</h2>
      {children && <p>{children}</p>}
      {action}
    </div>
  );
}

/** Ссылки на карточки компании на картах. */
export function CardLinks({ sources }: { sources: Place[] }) {
  const links = sources.filter((s) => s.card_url && s.provider && s.provider !== "manual");
  if (links.length === 0) return <span className="faint small">—</span>;
  return (
    <span className="row small" style={{ gap: 8 }}>
      {links.map((s) => (
        <a key={`${s.provider}-${s.provider_id}`} href={s.card_url!} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()}>
          {PROVIDER_LABEL[s.provider!]}
        </a>
      ))}
    </span>
  );
}

/** Кнопка с подтверждением вторым нажатием. */
export function ConfirmButton({ children, confirm, onConfirm, className }: { children: ReactNode; confirm: string; onConfirm: () => void; className?: string }) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const t = setTimeout(() => setArmed(false), 3000);
    return () => clearTimeout(t);
  }, [armed]);
  return (
    <button
      type="button"
      className={className ?? "btn btn-danger"}
      onClick={() => {
        if (armed) {
          setArmed(false);
          onConfirm();
        } else setArmed(true);
      }}
    >
      {armed ? confirm : children}
    </button>
  );
}

export function useToast(): [ReactNode, (msg: string) => void] {
  const [msg, setMsg] = useState<string | null>(null);
  useEffect(() => {
    if (!msg) return;
    const t = setTimeout(() => setMsg(null), 2500);
    return () => clearTimeout(t);
  }, [msg]);
  return [msg ? <div className="toast" role="status">{msg}</div> : null, setMsg];
}
