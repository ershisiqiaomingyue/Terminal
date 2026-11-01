import React, { useCallback, useEffect, useRef, useState } from "react";
import {
  CheckCircle2,
  ChevronRight,
  CircleAlert,
  FolderOpen,
  LoaderCircle,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  Trash2,
  Wrench,
} from "lucide-react";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
  Badge,
  Button,
  Input,
  useConfirmAction,
} from "rain-ui";
import {
  listLspServers,
  resetLspServers,
  saveLspServer,
  type LspConfigView,
  type LspServerView,
} from "@/platform/api";
import { formatInvokeError, isTauri, openFile } from "@/platform/runtime";
import { readChatNavSnapshot } from "@/views/Chat/chatNavStore";
import {
  collectLanguages,
  draftFrom,
  normalizeArgs,
  serverStatusLabel,
  serverStatusTone,
  type LanguageRow,
  type ServerDraft,
} from "./lspConfigDraft";
import { SettingsCard, SettingsPane } from "./settingsUI";
import { usePortal } from "rain-ui";
import "./lsp-panel.css";

const EXECUTABLE_FILTERS = [{ name: "语言服务器可执行文件", extensions: ["exe", "cmd", "bat", "ps1"] }];

function currentWorkspace() {
  return readChatNavSnapshot().workspacePath.trim();
}

function statusBadge(status: string) {
  return <Badge variant={serverStatusTone(status)}>{serverStatusLabel(status)}</Badge>;
}

const StatusIcon: React.FC<{ status: string }> = ({ status }) => {
  if (status === "ready") return <CheckCircle2 size={16} />;
  if (status === "invalid" || status === "missing" || status === "unlaunchable") return <CircleAlert size={16} />;
  return <Wrench size={16} />;
};

interface ArgumentRowsProps {
  args: string[];
  disabled?: boolean;
  onChange: (args: string[]) => void;
}

const ArgumentRows: React.FC<ArgumentRowsProps> = ({ args, disabled, onChange }) => (
  <div className="lsp-editors">
    {args.length === 0 && <span className="lsp-editor-empty">无启动参数</span>}
    {args.map((argument, index) => (
      <div className="lsp-field-row" key={index}>
        <Input
          value={argument}
          disabled={disabled}
          aria-label={`启动参数 ${index + 1}`}
          placeholder="例如 --stdio"
          onChange={(event) => onChange(args.map((item, position) => (position === index ? event.target.value : item)))}
        />
        <Button
          size="icon-sm"
          variant="ghost"
          disabled={disabled}
          icon={<Trash2 size={13} />}
          title={`删除第 ${index + 1} 个参数`}
          aria-label={`删除第 ${index + 1} 个参数`}
          onClick={() => onChange(args.filter((_, position) => position !== index))}
        />
      </div>
    ))}
    <div className="lsp-editor-actions">
      <Button variant="outline" disabled={disabled} icon={<Plus size={13} />} onClick={() => onChange([...args, ""])}>
        添加参数
      </Button>
    </div>
  </div>
);

interface LanguageRowsProps {
  rows: LanguageRow[];
  disabled?: boolean;
  onChange: (rows: LanguageRow[]) => void;
}

const LanguageRows: React.FC<LanguageRowsProps> = ({ rows, disabled, onChange }) => (
  <div className="lsp-editors">
    {rows.length === 0 && <span className="lsp-editor-empty">无语言映射</span>}
    {rows.map((row, index) => (
      <div className="lsp-field-row lsp-language-row" key={index}>
        <Input
          value={row.extension}
          disabled={disabled}
          aria-label={`扩展名 ${index + 1}`}
          placeholder="扩展名"
          onChange={(event) => onChange(rows.map((item, position) => (position === index ? { ...item, extension: event.target.value } : item)))}
        />
        <Input
          value={row.language}
          disabled={disabled}
          aria-label={`语言 ID ${index + 1}`}
          placeholder="语言 ID"
          onChange={(event) => onChange(rows.map((item, position) => (position === index ? { ...item, language: event.target.value } : item)))}
        />
        <Button
          size="icon-sm"
          variant="ghost"
          disabled={disabled}
          icon={<Trash2 size={13} />}
          title={`删除第 ${index + 1} 条语言映射`}
          aria-label={`删除第 ${index + 1} 条语言映射`}
          onClick={() => onChange(rows.filter((_, position) => position !== index))}
        />
      </div>
    ))}
    <div className="lsp-editor-actions">
      <Button variant="outline" disabled={disabled} icon={<Plus size={13} />} onClick={() => onChange([...rows, { extension: "", language: "" }])}>
        添加语言映射
      </Button>
    </div>
  </div>
);

export const LspPanel: React.FC = () => {
  const portal = usePortal();
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const [config, setConfig] = useState<LspConfigView | null>(null);
  const [drafts, setDrafts] = useState<Record<string, ServerDraft>>({});
  const [workspace, setWorkspace] = useState("");
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [savingId, setSavingId] = useState<string | null>(null);
  const [openServers, setOpenServers] = useState<string[]>([]);
  const initialRefreshStarted = useRef(false);
  const refreshRequestId = useRef(0);

  const applyConfig = (next: LspConfigView) => {
    setConfig(next);
    setDrafts(Object.fromEntries(next.servers.map((server) => [server.id, draftFrom(server)])));
  };

  const refresh = useCallback(async (announce = false) => {
    const requestId = ++refreshRequestId.current;
    const activeWorkspace = currentWorkspace();
    setWorkspace(activeWorkspace);
    if (!isTauri()) {
      setConfig(null);
      setDrafts({});
      setLoadError("请在桌面应用中查看语言服务器配置。");
      setLoading(false);
      return;
    }
    setLoading(true);
    try {
      const next = await listLspServers(activeWorkspace || null);
      if (requestId !== refreshRequestId.current) return;
      applyConfig(next);
      setLoadError(null);
      if (announce) portal("语言服务器已重新探测。", "success");
    } catch (reason) {
      if (requestId !== refreshRequestId.current) return;
      setConfig(null);
      setDrafts({});
      setLoadError(formatInvokeError(reason));
    } finally {
      if (requestId === refreshRequestId.current) setLoading(false);
    }
  }, [portal]);

  useEffect(() => {
    if (initialRefreshStarted.current) return;
    initialRefreshStarted.current = true;
    void refresh();
  }, [refresh]);

  const patchDraft = (id: string, patch: Partial<ServerDraft>) => {
    setDrafts((current) => ({ ...current, [id]: { ...current[id], ...patch } }));
  };

  const chooseExecutable = async (server: LspServerView) => {
    try {
      const path = await openFile(`选择 ${server.id} 的可执行文件`, EXECUTABLE_FILTERS);
      if (path) patchDraft(server.id, { command: path });
    } catch (reason) {
      portal(formatInvokeError(reason), "error");
    }
  };

  const saveServer = async (server: LspServerView) => {
    const draft = drafts[server.id];
    if (!draft || savingId) return;
    let languages: Record<string, string>;
    let args: string[];
    try {
      languages = collectLanguages(draft.languages);
      args = normalizeArgs(draft.args);
    } catch (reason) {
      portal(formatInvokeError(reason), "error");
      return;
    }
    setSavingId(server.id);
    try {
      applyConfig(await saveLspServer(
        { id: server.id, command: draft.command.trim(), args, languages },
        workspace || null,
      ));
      setLoadError(null);
      portal(`${server.id} 已保存。`, "success");
    } catch (reason) {
      portal(formatInvokeError(reason), "error");
    } finally {
      setSavingId(null);
    }
  };

  const resetToDefault = () =>
    confirmAction({
      title: "重置语言服务器配置",
      message: `将用内置默认配置覆盖 ${config?.configPath ?? "tools/lsp-servers.json"}，当前自定义配置会丢失。`,
      confirmLabel: "覆盖并重置",
      onConfirm: async () => {
        setLoading(true);
        try {
          applyConfig(await resetLspServers(workspace || null));
          setLoadError(null);
          portal("语言服务器配置已重置。", "success");
        } catch (reason) {
          portal(formatInvokeError(reason), "error");
        } finally {
          setLoading(false);
        }
      },
    });

  return (
    <SettingsPane
      title="语言服务器"
      action={
        config && !config.configError ? (
          <Button
            size="sm"
            variant="outline"
            icon={<RotateCcw size={14} />}
            loading={loading}
            onClick={resetToDefault}
          >
            恢复默认
          </Button>
        ) : undefined
      }
    >
      {loadError && (
        <SettingsCard className="settings-error-card">
          <div className="flex flex-col gap-1 px-[17px] py-3.5">
            <span className="text-[13px] font-medium text-foreground">读取失败</span>
            <span className="text-xs leading-5 text-muted-foreground">{loadError}</span>
          </div>
        </SettingsCard>
      )}

      {config?.configError && (
        <SettingsCard>
          <div className="flex items-start justify-between gap-5 px-[17px] py-4">
            <div className="flex min-w-0 flex-col gap-1">
              <span className="text-[13px] font-medium text-foreground">配置文件无法读取</span>
              <span className="break-all text-xs leading-5 text-muted-foreground">{config.configError}</span>
            </div>
            <Button size="sm" variant="outline" icon={<RotateCcw size={14} />} loading={loading} onClick={resetToDefault}>
              重置为默认
            </Button>
          </div>
        </SettingsCard>
      )}

      {confirmationDialog}

      {config && !config.configError && (
        <SettingsCard>
          {loading && !config.servers.length ? (
            <div className="flex items-center gap-2 px-[17px] py-5 text-xs text-muted-foreground">
              <LoaderCircle className="animate-spin" size={15} />正在探测语言服务器…
            </div>
          ) : (
            <Accordion type="multiple" value={openServers} onValueChange={setOpenServers}>
              {config.servers.map((server) => {
                const draft = drafts[server.id];
                const saving = savingId === server.id;
                const busy = savingId !== null;
                const languageSummary = server.languages
                  ? Object.keys(server.languages).slice(0, 5).join(" · ")
                  : "";
                return (
                  <AccordionItem value={server.id} key={server.id}>
                    <AccordionTrigger className="group px-[17px] py-3 hover:no-underline [&>svg:last-child]:hidden">
                      <div className="flex min-w-0 flex-1 items-center gap-3 text-left">
                        <span className={`lsp-status-icon is-${server.status}`}><StatusIcon status={server.status} /></span>
                        <span className="min-w-0 flex-1">
                          <span className="flex flex-wrap items-center gap-2">
                            <span className="text-[13px] font-medium text-foreground">{server.id}</span>
                            {statusBadge(server.status)}
                            {server.source && <Badge variant="primary">{server.source}</Badge>}
                          </span>
                          <span className="mt-1 block truncate text-xs font-normal text-muted-foreground">
                            {languageSummary || server.message}
                          </span>
                        </span>
                        <ChevronRight size={15} className="text-muted-foreground transition-transform duration-200 group-data-[state=open]:rotate-90" />
                      </div>
                    </AccordionTrigger>
                    <AccordionContent className="px-[17px] pb-4 pt-1">
                      <div className="lsp-expanded">
                        <div className="lsp-resolved-row">
                          <span className="lsp-expanded-label">解析路径</span>
                          <code>{server.resolvedPath || server.message}</code>
                        </div>

                        <div className="lsp-editor">
                          <div className="lsp-editor-head">
                            <span className="lsp-expanded-label">启动命令</span>
                            <span className="lsp-editor-hint">可填命令名或可执行文件绝对路径</span>
                          </div>
                          <div className="lsp-field-row">
                            <Input
                              value={draft?.command ?? ""}
                              disabled={busy}
                              aria-label={`${server.id} 启动命令`}
                              placeholder="例如 rust-analyzer"
                              onChange={(event) => patchDraft(server.id, { command: event.target.value })}
                            />
                            <Button
                              size="icon-sm"
                              variant="outline"
                              disabled={busy}
                              icon={<FolderOpen size={13} />}
                              title="选择本地可执行文件"
                              onClick={() => void chooseExecutable(server)}
                            />
                          </div>
                        </div>

                        <div className="lsp-editor">
                          <div className="lsp-editor-head">
                            <span className="lsp-expanded-label">启动参数</span>
                          </div>
                          <ArgumentRows
                            args={draft?.args ?? []}
                            disabled={busy}
                            onChange={(args) => patchDraft(server.id, { args })}
                          />
                        </div>

                        <div className="lsp-editor">
                          <div className="lsp-editor-head">
                            <span className="lsp-expanded-label">语言映射</span>
                            <span className="lsp-editor-hint">扩展名 → LSP 语言 ID</span>
                          </div>
                          <LanguageRows
                            rows={draft?.languages ?? []}
                            disabled={busy}
                            onChange={(languages) => patchDraft(server.id, { languages })}
                          />
                        </div>

                        {server.extraFields.length > 0 && (
                          <p className="lsp-extra-fields">保留字段：{server.extraFields.join("、")}</p>
                        )}

                        <div className="lsp-expanded-footer">
                          <code title={server.resolvedPath ?? undefined}>{server.resolvedPath || "未解析到可执行文件"}</code>
                          <div className="lsp-card-actions">
                            <Button
                              size="sm"
                              variant="ghost"
                              disabled={busy}
                              icon={<RotateCcw size={13} />}
                              onClick={() => patchDraft(server.id, draftFrom(server))}
                            >
                              还原
                            </Button>
                            <Button
                              size="sm"
                              variant="default"
                              loading={saving}
                              disabled={busy && !saving}
                              icon={<Save size={13} />}
                              onClick={() => void saveServer(server)}
                            >
                              保存
                            </Button>
                          </div>
                        </div>
                      </div>
                    </AccordionContent>
                  </AccordionItem>
                );
              })}
            </Accordion>
          )}
        </SettingsCard>
      )}
    </SettingsPane>
  );
};

export default LspPanel;


