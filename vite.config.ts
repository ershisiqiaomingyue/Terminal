import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  // Tauri 桌面端专用配置
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
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
