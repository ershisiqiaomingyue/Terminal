import React from "react";

/**
 * TerminalGlyphs
 * ---------------------------------------------------------------------------
 * 左栏「新建终端」菜单与终端卡片用的 5 个 16×16 自绘内联 SVG 图标。
 *
 * 为什么不复用 lucide：lucide 是统一线宽的通用图标，菜单里 PowerShell、cmd、
 * WSL、SSH 四者外形几乎一样，扫一眼分不出类型（需求 R7 / 验收 V2-09 明确
 * 要求「内联 SVG + 品牌色、不引入图标库」）。这里每个图标只用 3~6 个图元，
 * 靠**底色 + 一个特征形状**在 16px 下也能区分。
 *
 * 尺寸约定：viewBox 固定 0 0 16 16，默认渲染 16px；调用方要更小就传 size，
 * 形状按比例缩放（除 1px 描边外不做小数坐标）。
 */

export type TerminalGlyphKind = "pwsh" | "cmd" | "wsl" | "ssh" | "git";

interface TerminalGlyphProps {
  kind: TerminalGlyphKind;
  /** 渲染尺寸（正方形），默认 16。 */
  size?: number;
  className?: string;
}

/** 品牌底色：cmd 用 Windows 黑、pwsh 用 PowerShell 深蓝、其余用中性深色。 */
const BG: Record<TerminalGlyphKind, string> = {
  pwsh: "#012456",
  cmd: "#0C0C0C",
  wsl: "#1B1B1F",
  ssh: "#1E3A4C",
  git: "#2B1A14",
};

const GlyphShape: React.FC<{ kind: TerminalGlyphKind }> = ({ kind }) => {
  switch (kind) {
    case "pwsh":
      // 深蓝底 + 白色提示符 `>_`（PowerShell 的经典提示符形态）。
      return (
        <>
          <path d="M4.5 5.2 L7.6 8 L4.5 10.8" fill="none" stroke="#FFFFFF" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
          <path d="M8.6 11h3" fill="none" stroke="#FFFFFF" strokeWidth="1.4" strokeLinecap="round" />
        </>
      );
    case "cmd":
      // 黑底 + 灰色 `>_`，比 pwsh 更粗更钝，颜色也退一档。
      return (
        <>
          <path d="M4.2 5.4 L7 8 L4.2 10.6" fill="none" stroke="#D4D4D4" strokeWidth="1.3" strokeLinecap="square" />
          <path d="M8.4 11h3.4" fill="none" stroke="#D4D4D4" strokeWidth="1.3" strokeLinecap="square" />
        </>
      );
    case "wsl":
      // 企鹅（Tux）极简版：黑身 + 白肚 + 橙喙。
      return (
        <>
          <ellipse cx="8" cy="9" rx="4.2" ry="4.6" fill="#151515" stroke="#3A3A3A" strokeWidth="0.6" />
          <ellipse cx="8" cy="9.6" rx="2.4" ry="3.2" fill="#F5F5F5" />
          <circle cx="6.5" cy="5.4" r="0.8" fill="#F5F5F5" />
          <circle cx="9.5" cy="5.4" r="0.8" fill="#F5F5F5" />
          <path d="M8 6.2 l1.3 1.1 -1.3 1.1 -1.3 -1.1 Z" fill="#F7A21B" />
        </>
      );
    case "ssh":
      // 深青底 + 地球经纬（SSH 的语义是「远程」，地球比终端更有辨识度）。
      return (
        <>
          <circle cx="8" cy="8" r="4.6" fill="none" stroke="#6FC3E0" strokeWidth="1.1" />
          <path d="M8 3.4c2 2 2 7.2 0 9.2M8 3.4c-2 2-2 7.2 0 9.2" fill="none" stroke="#6FC3E0" strokeWidth="1" />
          <path d="M3.6 8h8.8" fill="none" stroke="#6FC3E0" strokeWidth="1" />
        </>
      );
    case "git":
      // git 分支：两圆一斜线 + 一根主干，橙色。
      return (
        <>
          <circle cx="5" cy="4.2" r="1.5" fill="none" stroke="#F05032" strokeWidth="1.2" />
          <circle cx="5" cy="12" r="1.5" fill="none" stroke="#F05032" strokeWidth="1.2" />
          <circle cx="11.2" cy="6.4" r="1.5" fill="none" stroke="#F05032" strokeWidth="1.2" />
          <path d="M5 5.7v4.8" fill="none" stroke="#F05032" strokeWidth="1.2" />
          <path d="M11.2 7.9c0 2-1.6 2.6-3.4 2.9" fill="none" stroke="#F05032" strokeWidth="1.2" strokeLinecap="round" />
        </>
      );
  }
};

export const TerminalGlyph: React.FC<TerminalGlyphProps> = ({ kind, size = 16, className }) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 16 16"
    role="img"
    aria-hidden="true"
    focusable="false"
    className={className}
    data-glyph={kind}
  >
    <rect x="0.5" y="0.5" width="15" height="15" rx="3.5" fill={BG[kind]} />
    <GlyphShape kind={kind} />
  </svg>
);

/** 各类型对应的图标 kind（本地终端按 shell 细分，WSL/SSH 直接映射）。 */
export const glyphForTabType = (
  type: "local" | "wsl" | "ssh",
  shell: "pwsh" | "cmd" = "pwsh",
): TerminalGlyphKind => {
  if (type === "local") return shell === "cmd" ? "cmd" : "pwsh";
  return type;
};
