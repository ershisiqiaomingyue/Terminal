import { useEffect, useRef, useState } from "react";
import { Maximize, Minus, Plus, X } from "lucide-react";
import { Button } from "rain-ui";
import "./chat-image-preview.css";

export function ChatImagePreview({ name, dataUrl, onClose }: { name: string; dataUrl: string; onClose: () => void }) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const [dimensions, setDimensions] = useState({ width: 0, height: 0 });
  const [bounds, setBounds] = useState({ width: 0, height: 0 });
  const [zoom, setZoom] = useState(1);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => { if (dialog?.open) dialog.close(); };
  }, []);
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const observer = new ResizeObserver(([entry]) => setBounds({ width: entry.contentRect.width, height: entry.contentRect.height }));
    observer.observe(stage);
    return () => observer.disconnect();
  }, []);
  const fit = dimensions.width && bounds.width
    ? Math.min(1, bounds.width / dimensions.width, bounds.height / dimensions.height) : 1;
  const scale = fit * zoom;
  return (
    <dialog ref={dialogRef} className="chat-image-preview" aria-label="图片预览"
      onCancel={(event) => { event.preventDefault(); onClose(); }}
      onClick={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <header className="chat-image-preview-header">
        <span className="chat-image-preview-name" title={name}>{name}</span>
        <div className="chat-image-preview-controls">
          <Button variant="ghost" title="缩小" aria-label="缩小" icon={<Minus size={16} />} disabled={zoom <= 0.5 || failed} onClick={() => setZoom((value) => Math.max(0.5, value / 1.25))} />
          <span className="chat-image-preview-scale">{Math.round(scale * 100)}%</span>
          <Button variant="ghost" title="放大" aria-label="放大" icon={<Plus size={16} />} disabled={zoom >= 8 || failed} onClick={() => setZoom((value) => Math.min(8, value * 1.25))} />
          <Button variant="ghost" title="适应窗口" aria-label="适应窗口" icon={<Maximize size={16} />} onClick={() => { setZoom(1); stageRef.current?.scrollTo(0, 0); }} />
          <Button variant="ghost" title="关闭预览" aria-label="关闭预览" icon={<X size={18} />} onClick={onClose} />
        </div>
      </header>
      <div ref={stageRef} className="chat-image-preview-stage">
        {failed ? <p role="alert">图片无法加载</p> : <div className="chat-image-preview-canvas">
          <img src={dataUrl} alt={name} draggable={false}
            style={dimensions.width ? { width: dimensions.width * scale, height: dimensions.height * scale } : { maxWidth: "100%", maxHeight: "100%" }}
            onLoad={(event) => setDimensions({ width: event.currentTarget.naturalWidth, height: event.currentTarget.naturalHeight })}
            onError={() => setFailed(true)} />
        </div>}
      </div>
    </dialog>
  );
}
