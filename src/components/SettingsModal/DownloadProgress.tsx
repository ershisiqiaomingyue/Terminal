import './download-progress.css';

export interface DownloadProgressProps {
  label: string;
  downloadedBytes: number;
  totalBytes?: number | null;
  bytesPerSecond: number;
  downloading: boolean;
  completed?: boolean;
}

function byteSize(bytes: number) {
  const value = Math.max(0, Number.isFinite(bytes) ? bytes : 0);
  if (value < 1024) return `${Math.floor(value)} B`;
  const index = Math.min(3, Math.floor(Math.log(value) / Math.log(1024)));
  return `${(value / 1024 ** index).toFixed(1)} ${['B', 'KB', 'MB', 'GB'][index]}`;
}

export function DownloadProgress({ label, downloadedBytes, totalBytes, bytesPerSecond, downloading, completed }: DownloadProgressProps) {
  const percent = completed ? 100 : downloading && totalBytes && totalBytes > 0
    ? Math.min(100, Math.max(0, downloadedBytes / totalBytes * 100)) : undefined;
  return <div className="plugin-download-progress">
    <div className="plugin-download-heading">
      <span className="plugin-download-label" role="status">{label}</span>
      {downloading && <span className="plugin-download-speed">{byteSize(bytesPerSecond)}/s</span>}
    </div>
    <div className="plugin-download-track" role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}>
      <span className={percent === undefined ? 'is-indeterminate' : undefined} style={percent === undefined ? undefined : { width: `${percent}%` }} />
    </div>
    {(downloading || downloadedBytes > 0) && <div className="plugin-download-details">
      <span>已下载 {byteSize(downloadedBytes)}{totalBytes && totalBytes > 0 ? ` / ${byteSize(totalBytes)}` : ''}</span>
      {percent !== undefined && <span>{Math.floor(percent)}%</span>}
    </div>}
  </div>;
}
