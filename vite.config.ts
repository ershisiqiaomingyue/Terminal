import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
      // rain-ui 源码组件库（workspace 外只读引用，构建期直接转译源码）。
      "rain-ui": path.resolve(__dirname, "../rain-ui/src/index.ts"),
    },
  },
  // Tauri 桌面端专用配置
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    fs: {
      // 允许 dev server 读取上级目录的 rain-ui 源码。
      allow: [".", ".."],
    },
  },
  build: {
    target: "esnext",
    minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_DEBUG,
    rollupOptions: {
      output: {
        manualChunks(id) {
          const normalized = id.replace(/\\/g, "/");
          if (!normalized.includes("/node_modules/")) return undefined;
          if (normalized.includes("/node_modules/react/")
            || normalized.includes("/node_modules/react-dom/")
            || normalized.includes("/node_modules/scheduler/")) return "vendor-react";
          if (normalized.includes("/node_modules/@tauri-apps/api/")) return "vendor-tauri";
          if (normalized.includes("/node_modules/@tauri-apps/plugin-")) return "vendor-tauri-plugins";
          if (normalized.includes("/node_modules/marked/")
            || normalized.includes("/node_modules/dompurify/")
            || normalized.includes("/node_modules/highlight.js/")) return "vendor-markdown";
          if (normalized.includes("/node_modules/@xterm/")) return "vendor-xterm";
          if (normalized.includes("/node_modules/@tanstack/react-virtual/")) return "vendor-virtual";
          if (normalized.includes("/node_modules/lucide-react/")) return "vendor-icons";
          // 图表运行时及其依赖保留为动态分包，避免普通对话加载图表引擎。
          return undefined;
        },
      },
    },
  },
});

