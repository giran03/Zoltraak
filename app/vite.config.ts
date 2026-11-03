import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Tauri drives the dev server, so the port is fixed and failures must be loud
// rather than silently falling back to another port the shell won't load.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1440,
    strictPort: true,
    proxy: {
      "/api": {
        target: "http://127.0.0.1:1450",
        changeOrigin: true,
      },
    },
    watch: {
      // The Rust side has its own rebuild loop; watching it doubles the work.
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    // Matches the webview floor Tauri v2 targets on each platform.
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
  },
});
