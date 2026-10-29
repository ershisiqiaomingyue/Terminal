import { Component, type ReactNode } from "react";
import { X } from "lucide-react";
import { Button } from "@/components/ui/Button";
import "./chat-file-preview.css";

export class ChatFilePreviewBoundary extends Component<{ children: ReactNode; onClose: () => void }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() { return { failed: true }; }

  render() {
    if (!this.state.failed) return this.props.children;
    return <aside className="chat-file-preview-panel" aria-label="文件预览失败">
      <header className="chat-file-preview-header"><div><strong>文件预览</strong></div><Button variant="subtle" size="xs" icon={<X size={16} />} title="关闭文件预览" aria-label="关闭文件预览" onClick={this.props.onClose} /></header>
      <p className="chat-file-preview-notice" role="alert">预览组件加载或渲染失败。请关闭后刷新页面重试，对话不受影响。</p>
    </aside>;
  }
}
