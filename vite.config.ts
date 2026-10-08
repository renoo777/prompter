import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 需要固定端口 + 固定失败行为，避免端口漂移导致壳找不到前端
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5273,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "chrome110",
    minify: "esbuild",
    sourcemap: false,
  },
});
