import { ArrowLeft } from "lucide-react";
import { Button } from "./Button";
import "../Button/tool-back-button.css";

export function ToolBackButton({ onBack }: { onBack?: () => void }) {
  if (!onBack) return null;
  return <Button type="button" className="tool-page-back-button" variant="secondary" size="sm"
    icon={<ArrowLeft size={17} />} aria-label="返回主页" title="返回主页" onClick={onBack} />;
}
