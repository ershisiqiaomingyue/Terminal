import DOMPurify from "dompurify";
import { diagramSourceError } from "./diagramSource";
import { BoundedTextCache } from "../../views/Chat/markdownCache";

let runtime: Promise<typeof import("mermaid")> | undefined;
let queue = Promise.resolve();
let sequence = 0;
const renderedDiagrams = new BoundedTextCache<string>(64, 4_000_000);

export function readDiagramPalette(element: HTMLElement) {
  const style = getComputedStyle(element);
  const context = document.createElement("canvas").getContext("2d")!;
  const color = (token: string, backdrop: string) => {
    context.clearRect(0, 0, 1, 1);
    context.fillStyle = backdrop;
    context.fillRect(0, 0, 1, 1);
    context.fillStyle = style.getPropertyValue(token).trim();
    context.fillRect(0, 0, 1, 1);
    return `#${Array.from(context.getImageData(0, 0, 1, 1).data).slice(0, 3).map((value) => value.toString(16).padStart(2, "0")).join("")}`;
  };
  // Mermaid needs opaque SVG colors. Composite translucent glass surfaces
  // before dropping alpha, so a faint white node does not become solid white.
  const canvas = color("--bg-canvas", "#ffffff");
  const surface = color("--markdown-code-bg", canvas);
  return { text: color("--text-primary", surface), muted: color("--control-text", surface), surface, node: color("--markdown-inline-code-bg", surface), font: style.getPropertyValue("--font-sans").trim() };
}

export type DiagramPalette = ReturnType<typeof readDiagramPalette>;

function diagramCacheKey(source: string, palette: DiagramPalette) {
  return JSON.stringify([source, palette.text, palette.muted, palette.surface, palette.node, palette.font]);
}

/** Cached SVGs are templates; each mounted view must isolate its own IDs. */
export function getCachedDiagram(source: string, palette: DiagramPalette) {
  return renderedDiagrams.get(diagramCacheKey(source, palette));
}

export function isolateDiagramIds(svg: string) {
  if (!svg) return "";
  const documentRef = new DOMParser().parseFromString(svg, "image/svg+xml");
  const ids = new Map<string, string>();
  const prefix = `diagram-view-${++sequence}-`;
  let index = 0;
  for (const element of documentRef.querySelectorAll("[id]")) {
    const id = `${prefix}${index++}`;
    if (!ids.has(element.id)) ids.set(element.id, id);
    element.id = id;
  }
  for (const element of documentRef.querySelectorAll("*")) {
    for (const attribute of Array.from(element.attributes)) {
      if (attribute.name === "id") continue;
      let value = attribute.value.replace(/url\(#([^)]*)\)/g, (match, id) => ids.has(id) ? `url(#${ids.get(id)})` : match);
      if (value.startsWith("#") && ids.has(value.slice(1))) value = `#${ids.get(value.slice(1))}`;
      if (attribute.name.startsWith("aria-")) value = value.split(" ").map((id) => ids.get(id) || id).join(" ");
      element.setAttribute(attribute.name, value);
    }
    if (element.localName === "style") {
      element.textContent = element.textContent?.replace(/#([\w-]+)/g, (match, id) => ids.has(id) ? `#${ids.get(id)}` : match) || "";
    }
  }
  return documentRef.documentElement.outerHTML;
}

export function renderDiagram(source: string, palette: DiagramPalette, active: () => boolean) {
  if (!active()) return Promise.resolve("");
  const cached = getCachedDiagram(source, palette);
  if (cached) return Promise.resolve(isolateDiagramIds(cached));
  const task = queue.then(async () => {
    if (!active()) return "";
    const error = diagramSourceError(source);
    if (error) throw new Error(error);
    const cached = getCachedDiagram(source, palette);
    if (cached) return isolateDiagramIds(cached);
    runtime ||= import("mermaid").catch((error) => { runtime = undefined; throw error; });
    const { default: mermaid } = await runtime;
    if (!active()) return "";
    mermaid.initialize({
      startOnLoad: false, securityLevel: "strict", suppressErrorRendering: true,
      maxTextSize: 50000, maxEdges: 2000, theme: "base", fontFamily: palette.font,
      htmlLabels: false,
      flowchart: { htmlLabels: false, useMaxWidth: true },
      themeVariables: {
        background: palette.surface, primaryColor: palette.node, primaryTextColor: palette.text,
        primaryBorderColor: palette.muted, secondaryColor: palette.surface, secondaryTextColor: palette.text,
        secondaryBorderColor: palette.muted, tertiaryColor: palette.node, tertiaryTextColor: palette.text,
        tertiaryBorderColor: palette.muted, lineColor: palette.muted, textColor: palette.text,
        mainBkg: palette.node, nodeBorder: palette.muted, clusterBkg: palette.surface,
        clusterBorder: palette.muted, edgeLabelBackground: palette.surface,
        actorBkg: palette.node, actorBorder: palette.muted, actorTextColor: palette.text,
        actorLineColor: palette.muted, signalColor: palette.text, signalTextColor: palette.text,
        labelBoxBkgColor: palette.node, labelBoxBorderColor: palette.muted, labelTextColor: palette.text,
        noteBkgColor: palette.node, noteBorderColor: palette.muted, noteTextColor: palette.text,
        activationBkgColor: palette.node, activationBorderColor: palette.muted,
      },
    });
    await mermaid.parse(source, { suppressErrors: false });
    if (!active()) return "";
    await document.fonts.ready;
    if (!active()) return "";
    // Mermaid measures live SVG text. Keep its staging DOM measurable without
    // exposing another diagram or changing the document's scroll dimensions.
    const staging = document.createElement("div");
    staging.setAttribute("aria-hidden", "true");
    staging.style.cssText = "position:fixed;left:-100000px;top:0;width:1600px;visibility:hidden;pointer-events:none;";
    document.body.append(staging);
    try {
      const { svg } = await mermaid.render(`markdown-diagram-${++sequence}`, source, staging);
      const clean = DOMPurify.sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true }, FORBID_TAGS: ["foreignObject", "script", "a"] });
      renderedDiagrams.set(diagramCacheKey(source, palette), clean, clean.length);
      return isolateDiagramIds(clean);
    } finally { staging.remove(); }
  });
  queue = task.then(() => undefined, () => undefined);
  return task;
}
