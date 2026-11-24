// Живые обновления скана по SSE: события сразу правят кэш запросов.
import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { Business, Scan, ScanEvent } from "./api";

export function useScanEvents(scanId: number | null) {
  const qc = useQueryClient();
  useEffect(() => {
    if (scanId == null) return;
    const es = new EventSource(`/api/events?scan_id=${scanId}`);
    // У большого скана компании проверяются десятками в секунду: правим кэш пачкой,
    // а не перерисовываем таблицу на каждое событие.
    let pending = new Map<number, Business>();
    let timer: ReturnType<typeof setTimeout> | null = null;
    const flush = () => {
      timer = null;
      const batch = pending;
      pending = new Map();
      qc.setQueryData<Business[]>(["businesses", scanId], (old) => {
        if (!old) return old;
        const next = old.slice();
        const index = new Map(next.map((b, i) => [b.id, i]));
        for (const b of batch.values()) {
          const i = index.get(b.id);
          if (i == null) next.push(b);
          else next[i] = b;
        }
        return next;
      });
    };
    es.onmessage = (e) => {
      const ev = JSON.parse(e.data) as ScanEvent;
      if (ev.type === "scan") {
        qc.setQueryData<{ scan: Scan; running: boolean }>(["scan", scanId], (old) => ({
          scan: ev.scan,
          running: ev.scan.status === "running" || ev.scan.status === "queued" || (old?.running ?? false),
        }));
        if (["done", "failed", "cancelled"].includes(ev.scan.status)) {
          qc.invalidateQueries({ queryKey: ["scan", scanId] });
          qc.invalidateQueries({ queryKey: ["businesses", scanId] });
          qc.invalidateQueries({ queryKey: ["overview", scanId] });
          qc.invalidateQueries({ queryKey: ["scans"] });
        } else if (ev.scan.stage === "audit") {
          // После поиска мест список компаний появился целиком.
          const have = qc.getQueryData<Business[]>(["businesses", scanId]);
          if (!have || have.length < ev.scan.total) qc.invalidateQueries({ queryKey: ["businesses", scanId] });
        }
      } else {
        pending.set(ev.business.id, ev.business);
        timer ??= setTimeout(flush, 400);
        qc.invalidateQueries({ queryKey: ["business", ev.business.id] });
      }
    };
    return () => {
      es.close();
      if (timer) clearTimeout(timer);
    };
  }, [scanId, qc]);
}
