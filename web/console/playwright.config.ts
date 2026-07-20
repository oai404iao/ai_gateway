import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright end-to-end config for the Console SPA.
 *
 * The SPA is served by the Vite dev server. Console API calls (`/console/v1/*`)
 * are fulfilled at the network layer inside each spec (see `e2e/mock-api.ts`),
 * so these tests exercise the real browser SPA without requiring the Rust
 * binary or PostgreSQL. Run with: pnpm --dir web/console e2e.
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: process.env.CI ? "line" : "list",
  use: {
    baseURL: "http://127.0.0.1:5174",
    trace: "on-first-retry",
    // The SPA is an English-first admin UI; pin a deterministic locale.
    locale: "en-US",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    command: "pnpm exec vite --config vite.e2e.config.ts",
    url: "http://127.0.0.1:5174",
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
});
