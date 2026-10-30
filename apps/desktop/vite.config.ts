import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// UI imports of "@contracts/ipc" (= ../../contracts/ipc.ts, see tsconfig paths) are type-only: erased at build, no alias needed.
// Tauri expects a fixed dev port (see src-tauri/tauri.conf.json -> build.devUrl).
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: "127.0.0.1" },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: "es2021",
    sourcemap: false,
  },
});
