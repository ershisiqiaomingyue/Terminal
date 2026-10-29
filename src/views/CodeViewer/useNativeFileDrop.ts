import { useEffect, useState } from "react";
import { isTauri } from "@/platform/runtime";
import type { BrowserTarget } from "./useCodeBrowser";

export function useNativeFileDrop({ active, target, connected, root, onLocal, onUpload }: {
  active: boolean; target: BrowserTarget; connected: boolean; root: string;
  onLocal: (path: string) => void; onUpload: (paths: string[], directory: string) => void;
}) {
  const [dropDirectory, setDropDirectory] = useState("");
  useEffect(() => {
    if (!active || !isTauri()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void import("@tauri-apps/api/webviewWindow").then(({ getCurrentWebviewWindow }) => getCurrentWebviewWindow().onDragDropEvent((event) => {
      if (disposed) return;
      const payload = event.payload;
      if (payload.type === "leave") { setDropDirectory(""); return; }
      const element = document.elementFromPoint(payload.position.x / window.devicePixelRatio, payload.position.y / window.devicePixelRatio);
      const directory = element?.closest<HTMLElement>("[data-code-directory]")?.dataset.codeDirectory;
      if (payload.type !== "drop") { setDropDirectory(target.kind === "ssh" && connected ? directory || "" : ""); return; }
      setDropDirectory("");
      if (target.kind === "ssh") { if (connected && root && directory) onUpload(payload.paths, directory); }
      else if (payload.paths[0]) onLocal(payload.paths[0]);
    })).then((stop) => { if (disposed) stop(); else unlisten = stop; }).catch(() => undefined);
    return () => { disposed = true; unlisten?.(); };
  }, [active, target, connected, root, onLocal, onUpload]);
  return dropDirectory;
}
