import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// В разработке API — на сервере сканера (cargo run), Vite проксирует /api.
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: { "/api": { target: "http://127.0.0.1:8787", changeOrigin: false } },
  },
  build: { outDir: "dist", chunkSizeWarningLimit: 1200 },
});
