import type { ReactNode } from "react";
import { SettingsEditDialog } from "./SettingsEditDialog";

export function ProviderEditDialog({ busy, onClose, children }: { busy: boolean; onClose: () => void; children: ReactNode }) {
  return <SettingsEditDialog title="编辑 Provider" busy={busy} onClose={onClose}>{children}</SettingsEditDialog>;
}
