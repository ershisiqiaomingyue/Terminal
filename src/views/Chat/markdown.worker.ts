import { projectLongMarkdown, type LongMarkdownRequest } from "./longMarkdownProjection";

self.onmessage = (event: MessageEvent<LongMarkdownRequest>) => {
  try {
    self.postMessage(projectLongMarkdown(event.data));
  } catch {
    self.postMessage({ error: true });
  }
};
