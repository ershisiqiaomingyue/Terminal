import { HelpCircle } from "lucide-react";
import { Modal } from "@/components/ui";
import { MarkdownMessage } from "@/views/Chat/MarkdownMessage";
import { HELP_CONTENT } from "./helpContent";
import "./help-dialog.css";

/**
 * 侧栏底部问号按钮打开的说明弹窗。
 *
 * 正文是 Markdown，用对话页同一个渲染器，省得再维护一套富文本样式。
 */
export function HelpDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  return (
    <Modal
      open={open}
      title="使用说明"
      description="功能总览与简要用法"
      icon={<HelpCircle size={16} />}
      className="help-modal-dialog"
      onClose={onClose}
    >
      <div className="help-dialog">
        <MarkdownMessage content={HELP_CONTENT} />
      </div>
    </Modal>
  );
}
