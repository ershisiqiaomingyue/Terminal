import React, { useCallback, useEffect, useRef, useState } from "react";
import {
  CheckCircle2,
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
import { Badge, Button, Input } from "@/components/ui";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
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
import { useSettingsNotification } from "./SettingsNotification";
import "./lsp-panel.css";

/** .exe/.cmd/.bat 可直接执行；.ps1 通过 PowerShell 主机启动。 */
const EXECUTABLE_FILTERS = [{ name: "语言服务器可执行文件", extensions: ["exe", "cmd", "bat", "ps1"] }];

/** 当前会话工作区：作为「工作区 node_modules/.bin」候选的搜索根。 */
function currentWorkspace() {
  return readChatNavSnapshot().workspacePath.trim();
}

function statusBadge(status: string) {
  return <Badge variant={serverStatusTone(status)}>{serverStatusLabel(status)}</Badge>;
}

const StatusIcon: React.FC<{ status: string }> = ({ status }) => {
  if (status === "ready") return <CheckCircle2 size={16} />;
  if (status === "invalid" || status === "missing" || status === "unlaunchable") {
    return <CircleAlert size={16} />;
  }
  return <Wrench size={16} />;
};

interface ArgumentRowsProps {
  args: string[];
  disabled?: boolean;
  onChange: (args: string[]) => void;
}

const ArgumentRows: React.FC<ArgumentRowsProps> = ({ args, disabled, onChange }) => (
  <div className="lsp-editors" style={{ marginTop: 0 }}>
    {args.length === 0 && <span className="lsp-editor-empty">无启动参数</span>}
    {args.map((argument, index) => (
      <div className="lsp-field-row" key={index}>
        <Input
          size="sm"
          value={argument}
          disabled={disabled}
          aria-label={`启动参数 ${index + 1}`}
          placeholder="例如 --stdio"
          onChange={(event) => onChange(args.map((item, position) => (position === index ? event.target.value : item)))}
        />
        <Button
          size="xs"
          variant="subtle"
          disabled={disabled}
          icon={<Trash2 size={13} />}
          title={`删除第 ${index + 1} 个参数`}
          aria-label={`删除第 ${index + 1} 个参数`}
          onClick={() => onChange(args.filter((_, position) => position !== index))}
        />
      </div>
    ))}
    <div className="lsp-editor-actions">
      <Button size="xs" variant="secondary" disabled={disabled} icon={<Plus size={13} />} onClick={() => onChange([...args, ""])}>
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
  <div className="lsp-editors" style={{ marginTop: 0 }}>
    {rows.length === 0 && <span className="lsp-editor-empty">无语言映射，该服务器永远不会被选中</span>}
    {rows.map((row, index) => (
      <div className="lsp-field-row lsp-language-row" key={index}>
        <Input
          size="sm"
          value={row.extension}
          disabled={disabled}
          aria-label={`扩展名 ${index + 1}`}
          placeholder="扩展名，例如 rs"
          onChange={(event) => onChange(rows.map((item, position) => (position === index ? { ...item, extension: event.target.value } : item)))}
        />
        <Input
          size="sm"
          value={row.language}
          disabled={disabled}
          aria-label={`语言 ID ${index + 1}`}
          placeholder="语言 ID，例如 rust"
          onChange={(event) => onChange(rows.map((item, position) => (position === index ? { ...item, language: event.target.value } : item)))}
        />
        <Button
          size="xs"
          variant="subtle"
          disabled={disabled}
          icon={<Trash2 size={13} />}
          title={`删除第 ${index + 1} 条语言映射`}
          aria-label={`删除第 ${index + 1} 条语言映射`}
          onClick={() => onChange(rows.filter((_, position) => position !== index))}
        />
      </div>
    ))}
    <div className="lsp-editor-actions">
      <Button
        size="xs"
        variant="secondary"
        disabled={disabled}
        icon={<Plus size={13} />}
        onClick={() => onChange([...rows, { extension: "", language: "" }])}
      >
        添加语言映射
      </Button>
    </div>
  </div>
);

export const LspPanel: React.FC = () => {
  const notify = useSettingsNotification();
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const [config, setConfig] = useState<LspConfigView | null>(null);
  const [drafts, setDrafts] = useState<Record<string, ServerDraft>>({});
  const [workspace, setWorkspace] = useState("");
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [savingId, setSavingId] = useState<string | null>(null);
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
      if (announce) notify("语言服务器已重新探测。", "success");
    } catch (reason) {
      if (requestId !== refreshRequestId.current) return;
      // 配置损坏不进可编辑状态：后端会拒绝写入，这里也不制造「已读取」的假象。
      setConfig(null);
      setDrafts({});
      setLoadError(formatInvokeError(reason));
    } finally {
      if (requestId === refreshRequestId.current) setLoading(false);
    }
  }, [notify]);

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
      notify(formatInvokeError(reason), "error");
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
      notify(formatInvokeError(reason), "error");
      return;
    }
    setSavingId(server.id);
    try {
      // 后端返回整份文件的最新视图，草稿直接由它重建，避免本地状态与文件漂移。
      applyConfig(await saveLspServer({ id: server.id, command: draft.command.trim(), args, languages }, workspace || null));
      setLoadError(null);
      notify(`${server.id} 已保存到语言服务器配置。`, "success");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      setSavingId(null);
    }
  };

  /**
   * 损坏或写错的 JSON 永远不会被自动覆盖；只有用户在这里明确确认后，
   * 后端才会用内置默认配置重写整个文件。
   */
  const resetToDefault = () =>
    confirmAction({
      title: "重置语言服务器配置",
      message: `将用内置默认配置覆盖 ${config?.configPath ?? "tools/lsp-servers.json"}：自定义的服务器、启动参数和语言映射都会丢失，且无法撤销。`,
      confirmLabel: "覆盖并重置",
      onConfirm: async () => {
        setLoading(true);
        try {
          applyConfig(await resetLspServers(workspace || null));
          setLoadError(null);
          notify("语言服务器配置已重置为内置默认值。", "success");
        } catch (reason) {
          notify(formatInvokeError(reason), "error");
        } finally {
          setLoading(false);
        }
      },
    });

  return (
    <section className="settings-pane">
      <header className="pane-header settings-environment-header">
        <div>
          <h2 className="pane-title">语言服务器（LSP）</h2>
          <p className="settings-environment-intro">
            为 lsp_diagnostics / lsp_symbols / lsp_definition / lsp_references 提供结构化结果。查找顺序：填写的绝对路径 → 工作区 node_modules/.bin → 系统 PATH。
          </p>
        </div>
        <Button size="sm" variant="secondary" icon={<RefreshCw size={14} />} loading={loading} onClick={() => void refresh(true)}>
          重新探测
        </Button>
      </header>

      {config?.configError && (
        <div className="setting-card-group">
          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">配置文件无法读取</span>
              <span className="setting-desc">{config.configError}</span>
              <span className="setting-desc">
                原文件保持不动：面板不会自动覆盖它。修好该 JSON 后点「重新探测」即可继续，或者用右侧的「重置为默认」明确丢弃它。
              </span>
            </div>
            <div className="setting-control-block">
              <Button
                size="sm"
                variant="secondary"
                icon={<RotateCcw size={14} />}
                loading={loading}
                onClick={resetToDefault}
              >
                重置为默认
              </Button>
            </div>
          </div>
        </div>
      )}

      {loadError && (
        <div className="setting-card-group">
          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">读取失败</span>
              <span className="setting-desc">{loadError}</span>
            </div>
          </div>
        </div>
      )}

      {confirmationDialog}

      {config && !config.configError && (
        <div>
          <div className="setting-group-title">服务器（{config.servers.length}）</div>
          <div className="setting-card-group settings-environment-list">
            {loading && !config.servers.length && (
              <div className="settings-environment-loading"><LoaderCircle size={16} />正在探测语言服务器...</div>
            )}
            {config.servers.map((server) => {
              const draft = drafts[server.id];
              const saving = savingId === server.id;
              const busy = savingId !== null;
              return (
                <article className="settings-environment-tool" key={server.id}>
                  <div className={`settings-environment-tool-icon is-${server.status}`}>
                    <StatusIcon status={server.status} />
                  </div>
                  <div className="settings-environment-tool-main">
                    <div className="settings-environment-tool-title">
                      <strong>{server.id}</strong>
                      {statusBadge(server.status)}
                      {server.source && <Badge variant="primary">{server.source}</Badge>}
                    </div>
                    <span>{server.message}</span>
                    {server.resolvedPath && <code title={server.resolvedPath}>{server.resolvedPath}</code>}
                    {server.extraFields.length > 0 && (
                      <div className="settings-environment-tool-meta">
                        <span title={server.extraFields.join("、")}>保存时原样保留字段：{server.extraFields.join("、")}</span>
                      </div>
                    )}

                    <div className="lsp-editors">
                      <div className="lsp-editor">
                        <div className="lsp-editor-head">
                          <span className="setting-title">启动命令</span>
                          <span className="setting-desc">可填 PATH 中的命令名，也可选本地可执行文件填绝对路径；相对路径按文件所在目录解析。</span>
                        </div>
                        <div className="lsp-field-row">
                          <Input
                            size="sm"
                            value={draft?.command ?? ""}
                            disabled={busy}
                            aria-label={`${server.id} 启动命令`}
                            placeholder="例如 rust-analyzer"
                            onChange={(event) => patchDraft(server.id, { command: event.target.value })}
                          />
                          <Button
                            size="xs"
                            variant="subtle"
                            disabled={busy}
                            icon={<FolderOpen size={13} />}
                            title="选择本地可执行文件（.exe / .cmd / .bat / .ps1）"
                            onClick={() => void chooseExecutable(server)}
                          >
                            选择
                          </Button>
                        </div>
                      </div>

                      <div className="lsp-editor">
                        <div className="lsp-editor-head">
                          <span className="setting-title">启动参数</span>
                          <span className="setting-desc">每行一个参数，按原样传给语言服务器进程。</span>
                        </div>
                        <ArgumentRows
                          args={draft?.args ?? []}
                          disabled={busy}
                          onChange={(args) => patchDraft(server.id, { args })}
                        />
                      </div>

                      <div className="lsp-editor">
                        <div className="lsp-editor-head">
                          <span className="setting-title">语言映射</span>
                          <span className="setting-desc">扩展名（不带点）→ LSP 语言 ID，决定哪些文件交给该服务器。</span>
                        </div>
                        <LanguageRows
                          rows={draft?.languages ?? []}
                          disabled={busy}
                          onChange={(languages) => patchDraft(server.id, { languages })}
                        />
                      </div>
                    </div>
                  </div>
                  <div className="settings-environment-tool-actions">
                    <div className="lsp-card-actions">
                      <Button
                        size="xs"
                        variant="subtle"
                        disabled={busy}
                        icon={<RotateCcw size={13} />}
                        title="放弃未保存的修改"
                        onClick={() => patchDraft(server.id, draftFrom(server))}
                      >
                        还原
                      </Button>
                      <Button
                        size="xs"
                        variant="primary"
                        loading={saving}
                        disabled={busy && !saving}
                        icon={<Save size={13} />}
                        onClick={() => void saveServer(server)}
                      >
                        保存
                      </Button>
                    </div>
                  </div>
                </article>
              );
            })}
          </div>
        </div>
      )}

      <p className="settings-environment-footnote">
        {config?.usingBundledDefault
          ? `尚未创建配置文件（${config.configPath}），当前展示内置默认服务器；保存任意一项会创建该文件，其余条目按默认值写入。`
          : config
            ? `配置文件：${config.configPath}；未编辑的条目与未知字段会原样保留。`
            : "配置文件：tools/lsp-servers.json"}
        {workspace ? `当前工作区：${workspace}（会在其 node_modules/.bin 中优先查找）。` : "未选择工作区，因此不会查找项目本地的 node_modules/.bin。"}
      </p>
      <p className="settings-environment-footnote">
        .ps1 脚本会用 PowerShell 主机启动；未找到的服务器只影响这 4 个 LSP 工具，其他工具仍可用源码检索。
      </p>
    </section>
  );
};

export default LspPanel;
