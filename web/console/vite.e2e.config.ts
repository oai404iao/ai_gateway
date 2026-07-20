import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

/**
 * Vite config used only by Playwright e2e (see playwright.config.ts).
 *
 * Unlike the default dev config, this serves the SPA over plain HTTP on
 * 127.0.0.1 without the /console/v1 proxy: Playwright's webServer readiness
 * probe cannot ignore the self-signed HTTPS cert the dev config relies on,
 * and e2e specs fulfill /console/v1/* at the network layer instead of
 * proxying to the Rust binary. Cookie/Secure-attribute behavior is covered by
 * the integration and component test suites; e2e focuses on browser-shell UX.
 */
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  server: {
    host: "127.0.0.1",
    port: 5174,
    strictPort: true,
  },
});
