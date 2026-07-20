import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  server: {
    // Local dev uses an HTTPS same-origin Console hostname so the
    // __Host- prefixed refresh cookie and its Secure attribute behave like
    // production. Run the gateway Console listener on 127.0.0.1:3001 and
    // browse https://console.localhost:5173; /console/v1/* is proxied to it.
    host: "console.localhost",
    https: {},
    port: 5173,
    proxy: {
      "/console/v1": {
        target: "http://127.0.0.1:3001",
        changeOrigin: false,
      },
    },
  },
});
