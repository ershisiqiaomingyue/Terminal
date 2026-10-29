import React, { useState } from "react";
import { Settings2 } from "lucide-react";
import openai from "@/assets/providers/openai.svg";
import anthropic from "@/assets/providers/claude.png";
import deepseek from "@/assets/providers/deepseek.png";
import kimi from "@/assets/providers/kimi.png";
import glm from "@/assets/providers/glm.svg";
import qwen from "@/assets/providers/qwen.png";
import mimo from "@/assets/providers/mimo.png";
import gemini from "@/assets/providers/gemini.png";
import grok from "@/assets/providers/grok.png";
import workbuddy from "@/assets/providers/workbuddy.svg";
import workbuddyHunyuan from "@/assets/providers/workbuddy-hunyuan.svg";
import workbuddyDeepseek from "@/assets/providers/workbuddy-deepseek.svg";
import workbuddyGlm from "@/assets/providers/workbuddy-glm.svg";
import workbuddyMinimax from "@/assets/providers/workbuddy-minimax.svg";
import workbuddyKimi from "@/assets/providers/workbuddy-kimi.svg";
import "./provider-icon.css";

export type ProviderBrand =
  | "openai"
  | "anthropic"
  | "deepseek"
  | "kimi"
  | "glm"
  | "qwen"
  | "mimo"
  | "gemini"
  | "grok"
  | "workbuddy"
  | "custom";

interface ProviderIconProps {
  brand: ProviderBrand | string;
  size?: number;
}

// WorkBuddy assets come from its official desktop application's renderer.
const OFFICIAL_ICONS: Record<string, string> = { openai, anthropic, deepseek, kimi, glm, qwen, mimo, gemini, grok, workbuddy,
  "workbuddy-hunyuan": workbuddyHunyuan, "workbuddy-deepseek": workbuddyDeepseek,
  "workbuddy-glm": workbuddyGlm, "workbuddy-minimax": workbuddyMinimax, "workbuddy-kimi": workbuddyKimi };

export const ProviderIcon: React.FC<ProviderIconProps> = ({ brand, size = 18 }) => {
  const source = OFFICIAL_ICONS[brand as ProviderBrand];
  const monochrome = brand === "openai" || brand === "deepseek" || brand.startsWith("workbuddy-");
  const [failedSource, setFailedSource] = useState("");
  return <span className="provider-icon" data-brand={brand} aria-hidden="true" style={{ display: "inline-flex", alignItems: "center", justifyContent: "center", width: size, height: size, flex: `0 0 ${size}px`, lineHeight: 0, borderRadius: 4, overflow: "hidden", background: source ? "transparent" : "var(--bg-hover)", color: "var(--text-secondary)" }}>
    {monochrome
      ? <span className="provider-icon-monochrome" style={{ maskImage: `url("${source}")`, WebkitMaskImage: `url("${source}")` }} />
      : source && failedSource !== source
      ? <img src={source} alt="" width={size} height={size} draggable={false} style={{ display: "block", width: "100%", height: "100%", objectFit: "contain" }} onError={() => setFailedSource(source)} />
      : brand && brand !== 'custom'
      ? <span style={{ fontSize: size * .7, fontWeight: 700, color: 'var(--accent-primary)' }}>{brand.slice(0, 1).toUpperCase()}</span>
      : <Settings2 size={size - 3} />}
  </span>;
};

export default ProviderIcon;
