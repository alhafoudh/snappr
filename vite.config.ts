import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri drives this dev server, so the port is fixed and failures must be loud:
// if vite silently moves to another port, the webview loads nothing and the app
// comes up blank with no clue why.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5183,
    strictPort: true,
    watch: {
      // The bundled runtime is ~87 MB of Ruby and dylibs and the Rust target dir
      // is larger again. Watching either pins a core for nothing.
      ignored: ["**/build/**", "**/src-tauri/**", "**/vendor/**"],
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "safari15",
  },
});
