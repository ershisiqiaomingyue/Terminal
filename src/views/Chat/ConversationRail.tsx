import { memo, useCallback, useLayoutEffect, useRef, useState } from "react";

type Marker = { id: string; preview: string };
type Preview = { id: string; top: number };

export const ConversationRail = memo(function ConversationRail({ markers, activeId, onNavigate }: {
  markers: Marker[];
  activeId: string;
  onNavigate: (id: string) => void;
}) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  const buttonsRef = useRef(new Map<string, HTMLButtonElement>());
  const [edges, setEdges] = useState({ above: false, below: false });
  const [preview, setPreview] = useState<Preview | null>(null);

  const syncEdges = useCallback(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const above = viewport.scrollTop > 1;
    const below = viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop > 1;
    setEdges((previous) => previous.above === above && previous.below === below ? previous : { above, below });
  }, []);

  const revealMarker = useCallback((id: string, center = false) => {
    const viewport = viewportRef.current;
    const button = buttonsRef.current.get(id);
    if (!viewport || !button) return;
    const bounds = viewport.getBoundingClientRect();
    const row = button.getBoundingClientRect();
    const inset = Math.min(24, viewport.clientHeight / 4);
    if (center || row.top < bounds.top + inset || row.bottom > bounds.bottom - inset) {
      viewport.scrollTop += row.top - bounds.top + row.height / 2 - viewport.clientHeight / 2;
    }
    syncEdges();
  }, [syncEdges]);

  useLayoutEffect(() => {
    setPreview(null);
    if (activeId) revealMarker(activeId, true);
    else syncEdges();
  }, [activeId, markers, revealMarker, syncEdges]);

  useLayoutEffect(() => {
    const viewport = viewportRef.current;
    const track = trackRef.current;
    if (!viewport || !track) return;
    const observer = new ResizeObserver(() => {
      if (activeId) revealMarker(activeId);
      syncEdges();
    });
    observer.observe(viewport);
    observer.observe(track);
    return () => observer.disconnect();
  }, [activeId, revealMarker, syncEdges]);

  const showPreview = (marker: Marker, button: HTMLButtonElement) => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const row = button.getBoundingClientRect();
    const top = row.top - viewport.getBoundingClientRect().top + row.height / 2;
    setPreview({ id: marker.id, top: Math.max(20, Math.min(viewport.clientHeight - 20, top)) });
  };
  const hoveredIndex = preview ? markers.findIndex((marker) => marker.id === preview.id) : -1;
  const hovered = hoveredIndex >= 0 ? markers[hoveredIndex] : null;

  return <nav className="conversation-rail" aria-label="用户输入导航" onMouseLeave={() => setPreview(null)}>
    <div ref={viewportRef} className="conversation-rail-viewport" data-overflow-above={edges.above} data-overflow-below={edges.below} onScroll={() => { syncEdges(); setPreview(null); }}>
      <div ref={trackRef} className="conversation-rail-track">
        {markers.map((marker, index) => {
          const distanceClass = hoveredIndex < 0 ? "" : ` rail-distance-${Math.min(Math.abs(index - hoveredIndex), 4)}`;
          return <button key={marker.id} ref={(button) => {
            if (button) buttonsRef.current.set(marker.id, button);
            else buttonsRef.current.delete(marker.id);
          }} type="button" className={`conversation-rail-marker${activeId === marker.id ? " active" : ""}${distanceClass}`} aria-current={activeId === marker.id ? "location" : undefined} aria-label={`跳转到输入：${marker.preview}`} onMouseEnter={(event) => showPreview(marker, event.currentTarget)} onFocus={(event) => { revealMarker(marker.id); showPreview(marker, event.currentTarget); }} onBlur={() => setPreview(null)} onClick={() => { revealMarker(marker.id); onNavigate(marker.id); }}>
            <span className="conversation-rail-dot" aria-hidden="true" />
          </button>;
        })}
      </div>
    </div>
    {hovered && preview && <span className="conversation-rail-preview" style={{ top: preview.top }}><b>{hovered.preview}</b></span>}
  </nav>;
});
