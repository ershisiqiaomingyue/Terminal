import { invoke } from "./runtime";

export interface DiagnosticLogInfo {
  directory: string;
  runId: string;
  enabled: boolean;
  lastError: string | null;
  fileCount: number;
  totalBytes: number;
  maxFileBytes: number;
  maxFiles: number;
  retentionDays: number;
}
export const getDiagnosticLogInfo = () => invoke<DiagnosticLogInfo>("get_diagnostic_log_info");
export const openDiagnosticLogDirectory = () => invoke<void>("open_diagnostic_log_directory");
export const exportDiagnosticLogs = () => invoke<string>("export_diagnostic_logs");
