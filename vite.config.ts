import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const ui = resolve(import.meta.dirname, "ui");

export default defineConfig({
  root: ui,
  plugins: [react(), tailwindcss()],
  // Biarkan log Rust dari `tauri dev` tetap terlihat.
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    outDir: resolve(import.meta.dirname, "dist"),
    emptyOutDir: true,
    // WKWebView macOS 13.3+ dan WebView2.
    target: ["safari16.4", "chrome110"],
    rolldownOptions: {
      input: {
        main: resolve(ui, "index.html"),
        viewer: resolve(ui, "viewer.html"),
      },
    },
  },
});
