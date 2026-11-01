import { useEffect, useRef, useState } from "react";
import { Check, Download, LoaderCircle, Upload, X } from "lucide-react";
import { Button } from "rain-ui";
import { Card } from "rain-ui";
import { invoke, formatInvokeError } from "@/platform/runtime";
import { useFileTransfers, formatTransferBytes, type FileTransfer } from "./fileTransfers";
import "./file-transfers.css";
import { downloadErrorMessage } from "./downloadAuthorization";

const phases = { checking: "检查大小", compressing: "压缩中", transferring: "传输中", extracting: "解压中", completed: "已完成" };

export function useUploadCompletion(onComplete: (job: FileTransfer) => void) {
  const jobs = useFileTransfers();
  const seen = useRef(new Map(jobs.map((job) => [job.id, job.status])));
  useEffect(() => {
    for (const job of jobs) {
      if (job.status === "completed" && job.direction === "upload" && seen.current.get(job.id) !== "completed") onComplete(job);
    }
    seen.current = new Map(jobs.map((job) => [job.id, job.status]));
  }, [jobs, onComplete]);
}

export function TransferProgressRows({ jobs }: { jobs: FileTransfer[] }) {
  const [clock, setClock] = useState(Date.now());
  const [error, setError] = useState("");
  const [cancelling, setCancelling] = useState<string[]>([]);
  const running = jobs.some((job) => job.status === "running");
  useEffect(() => {
    if (!running) return;
    setClock(Date.now()); const timer = setInterval(() => setClock(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running]);
  const cancel = async (id: string) => {
    setCancelling((previous) => [...previous, id]); setError("");
    try { await invoke("cancel_code_transfer", { id }); }
    catch (reason) { setError(formatInvokeError(reason)); }
    finally { setCancelling((previous) => previous.filter((item) => item !== id)); }
  };
  return <>
    {jobs.map((job) => {
      const running = job.status === "running";
      const percent = job.bytesTotal ? Math.min(100, Math.round(job.bytesDone / job.bytesTotal * 100)) : undefined;
      const seconds = Math.max(0, Math.floor(((job.finishedAt || clock) - job.startedAt) / 1000));
      const status = running ? phases[job.phase] : job.status === "completed" ? "已完成" : job.status === "cancelled" ? "已取消" : "失败";
      const progressKnown = job.phase === "transferring" || job.phase === "extracting";
      return <Card key={job.id} tone="muted" className={`code-transfer-card code-transfer-job code-download-pending is-${job.status}`}>
        <span className="code-transfer-direction" aria-hidden="true">{job.direction === "download" ? <Download size={16} /> : <Upload size={16} />}</span>
        <span className="code-transfer-card-content"><strong title={job.name}>{job.name}</strong><small>{job.server} · {status} · {seconds}s</small>
          {running && <><progress aria-label={`${job.name} ${job.direction === "download" ? "下载" : "上传"}进度`} max={100} value={progressKnown ? percent : undefined} /><small>{progressKnown && percent != null ? `${formatTransferBytes(job.bytesDone)} / ${formatTransferBytes(job.bytesTotal)} · ${percent}%` : status}</small></>}
          {job.outputPath && <small title={job.filePath || job.outputPath} className="code-transfer-path">{job.filePath || job.outputPath}</small>}
          {job.error && <small className="code-download-error">{downloadErrorMessage(job.error)}</small>}
        </span>
        <span className="code-transfer-card-actions">{running ? <><LoaderCircle size={14} className="inline-spin" aria-label="正在传输" /><Button variant="ghost" icon={<X size={14} />} aria-label={`取消${job.direction === "upload" ? "上传" : "下载"} ${job.name}`} disabled={cancelling.includes(job.id)} onClick={() => void cancel(job.id)} /></> : job.status === "completed" ? <Check size={14} className="code-download-success" aria-label="已完成" /> : <X size={14} aria-label={status} />}</span>
      </Card>;
    })}
    {error && <p className="code-download-error" role="alert">{error}</p>}
  </>;
}
