// Карты на Leaflet: выбор места скана и результаты.
import { useEffect, useMemo, useRef } from "react";
import L from "leaflet";
import { Circle, CircleMarker, MapContainer, Marker, Popup, TileLayer, Tooltip, useMap, useMapEvents } from "react-leaflet";
import type { Business, LatLon } from "./api";
import { hostOf, LEAD_LABEL } from "./format";

const TILES = "https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png";
const ATTR = '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a>';

const pinIcon = L.divIcon({ className: "", html: '<div class="pin"></div>', iconSize: [18, 18], iconAnchor: [9, 9] });

function zoomFor(radius: number): number {
  return Math.max(10, Math.min(17, Math.round(15.6 - Math.log2(radius / 500))));
}

function FlyTo({ center, radius }: { center: LatLon; radius: number }) {
  const map = useMap();
  useEffect(() => {
    map.flyTo([center.lat, center.lon], zoomFor(radius), { duration: 0.6 });
    // Только при смене центра извне: радиус меняет масштаб отдельно.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [center.lat, center.lon]);
  return null;
}

function ClickToSet({ onPick }: { onPick: (p: LatLon) => void }) {
  useMapEvents({ click: (e) => onPick({ lat: e.latlng.lat, lon: e.latlng.lng }) });
  return null;
}

export function MapPicker({ center, radius, onPick, flyKey }: { center: LatLon; radius: number; onPick: (p: LatLon) => void; flyKey: number }) {
  return (
    <MapContainer className="map" center={[center.lat, center.lon]} zoom={zoomFor(radius)} scrollWheelZoom>
      <TileLayer url={TILES} attribution={ATTR} />
      <ClickToSet onPick={onPick} />
      <FlyTo key={flyKey} center={center} radius={radius} />
      <Circle center={[center.lat, center.lon]} radius={radius} pathOptions={{ color: "#cf4528", weight: 2, fillOpacity: 0.08 }} />
      <Marker
        position={[center.lat, center.lon]}
        icon={pinIcon}
        draggable
        eventHandlers={{
          dragend: (e) => {
            const ll = (e.target as L.Marker).getLatLng();
            onPick({ lat: ll.lat, lon: ll.lng });
          },
        }}
      />
    </MapContainer>
  );
}

const LEAD_COLOR: Record<string, string> = {
  hot: "#cf4528",
  warm: "#c4870f",
  cold: "#5b6a61",
  closed: "#8b929b",
  pending: "#8b929b",
};

function FitTo({ points, fallback }: { points: [number, number][]; fallback: LatLon }) {
  const map = useMap();
  const key = points.length;
  useEffect(() => {
    if (points.length >= 2) map.fitBounds(L.latLngBounds(points), { padding: [24, 24], maxZoom: 16 });
    else map.setView([fallback.lat, fallback.lon], 14);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return null;
}

/** Больше точек Leaflet рисует медленно; остальные видны в таблице. */
const MAX_MARKERS = 5000;

export function ResultsMap({
  items,
  center,
  radius,
  selected,
  onSelect,
}: {
  items: Business[];
  center: LatLon | null;
  radius: number;
  selected: number | null;
  onSelect: (id: number) => void;
}) {
  const located = useMemo(() => items.filter((b) => b.lat != null && b.lon != null).slice(0, MAX_MARKERS), [items]);
  const points = useMemo(() => located.map((b) => [b.lat!, b.lon!] as [number, number]), [located]);
  const fallback = center ?? { lat: 55.751, lon: 37.618 };
  const sel = selected != null ? located.find((b) => b.id === selected) : undefined;
  const onSelectRef = useRef(onSelect);
  onSelectRef.current = onSelect;
  // Маркеры не зависят от выделения: у большого скана тысячи точек, и перерисовывать их
  // на каждое наведение на строку таблицы нельзя. Выделенная точка — отдельным слоем сверху.
  const markers = useMemo(() => {
    const light = located.length > 1500;
    return located.map((b) => (
      <CircleMarker
        key={b.id}
        center={[b.lat!, b.lon!]}
        radius={light ? 5 : 7}
        pathOptions={{ color: "#ffffff", weight: light ? 1 : 1.5, fillColor: LEAD_COLOR[b.lead ?? "pending"], fillOpacity: 0.95 }}
        eventHandlers={{ click: () => onSelectRef.current(b.id) }}
      >
        {!light && <Tooltip direction="top" offset={[0, -6]}>{b.name}</Tooltip>}
        {!light && (
          <Popup>
            <b>{b.name}</b>
            <br />
            {b.lead ? LEAD_LABEL[b.lead] : "ждёт проверки"}
            {b.overall != null && ` · ${Math.round(b.overall)}/100`}
            <br />
            {b.website ? hostOf(b.website) : "нет сайта"}
          </Popup>
        )}
      </CircleMarker>
    ));
  }, [located]);
  return (
    <MapContainer className="map" center={[fallback.lat, fallback.lon]} zoom={14} scrollWheelZoom preferCanvas>
      <TileLayer url={TILES} attribution={ATTR} />
      <FitTo points={points} fallback={fallback} />
      {center && radius > 0 && <Circle center={[center.lat, center.lon]} radius={radius} pathOptions={{ color: "#8b929b", weight: 1, fillOpacity: 0.03, dashArray: "4 4" }} />}
      {markers}
      {sel && (
        <CircleMarker
          center={[sel.lat!, sel.lon!]}
          radius={10}
          pathOptions={{ color: "#15181c", weight: 3, fillColor: LEAD_COLOR[sel.lead ?? "pending"], fillOpacity: 0.95 }}
          eventHandlers={{ click: () => onSelect(sel.id) }}
        >
          <Tooltip direction="top" offset={[0, -8]} permanent>{sel.name}</Tooltip>
        </CircleMarker>
      )}
    </MapContainer>
  );
}
