import { expect, test } from "@playwright/test";
import { mockConsoleApi } from "./mock-api";

for (const reducedMotion of ["reduce", "no-preference"] as const) {
  test(`loading indicator respects ${reducedMotion}`, async ({ page }) => {
    await page.emulateMedia({ reducedMotion });
    await page.route("**/console/v1/auth/refresh", () => {});
    await page.goto("/login");
    const spinner = page.getByRole("status", { name: "Loading" });
    await expect(spinner).toBeVisible();

    if (reducedMotion === "no-preference") {
      await expect.poll(() => spinner.evaluate((element) => getComputedStyle(element).transform))
        .not.toBe("none");
    }
    const transforms = await spinner.evaluate(async (element) => {
      const samples = new Set<string>();
      for (let i = 0; i < 12; i++) {
        await new Promise(requestAnimationFrame);
        samples.add(getComputedStyle(element).transform);
      }
      return samples.size;
    });
    if (reducedMotion === "reduce") expect(transforms).toBe(1);
    else expect(transforms).toBeGreaterThan(1);

    await page.emulateMedia({ reducedMotion: "reduce" });
    await expect.poll(() => spinner.evaluate((element) => getComputedStyle(element).transform))
      .toBe("none");
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await expect.poll(() => spinner.evaluate((element) => getComputedStyle(element).transform))
      .not.toBe("none");
  });
}

test("pending content stops and resumes its skeleton when motion preference changes", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await mockConsoleApi(page);
  await page.route("**/console/v1/me", () => {});
  await page.goto("/login");
  await page.getByLabel(/email/i).fill("admin@example.com");
  await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
  await page.getByRole("button", { name: /sign in/i }).click();
  const skeleton = page.locator('[data-slot="skeleton"]').first();
  await expect(skeleton).toBeVisible();
  await expect(skeleton).toHaveCSS("opacity", "1");
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await expect.poll(() => skeleton.evaluate((element) => getComputedStyle(element).opacity))
    .not.toBe("1");
  await page.emulateMedia({ reducedMotion: "reduce" });
  await expect(skeleton).toHaveCSS("opacity", "1");
});

test("theme preference also controls toast appearance and survives reload", async ({ page }) => {
  await mockConsoleApi(page);
  await page.goto("/login");
  await page.getByLabel(/email/i).fill("admin@example.com");
  await page.getByLabel(/^password$/i).fill("correct-horse-battery-staple");
  await page.getByRole("button", { name: /sign in/i }).click();
  await expect(page).toHaveURL(/\/account/);

  await page.getByRole("button", { name: "Toggle theme" }).click();
  await page.getByRole("menuitemradio", { name: "Dark", exact: true }).click();
  await expect(page.locator("html")).toHaveClass(/dark/);
  await expect(page.locator("[data-sonner-toaster]")).toHaveAttribute("data-sonner-theme", "dark");
  await page.reload();
  await expect(page.locator("html")).toHaveClass(/dark/);
  await expect(page.getByRole("heading", { name: "Profile", exact: true })).toBeVisible();

  await page.getByRole("button", { name: /Initial Admin/ }).click();
  await page.getByRole("menuitem", { name: "Sign out", exact: true }).click();
  await expect(page).toHaveURL(/\/login/);
  const storage = await page.evaluate(() => ({
    local: Object.entries(localStorage),
    session: Object.entries(sessionStorage),
  }));
  expect(JSON.stringify(storage)).not.toContain("e2e-mock-access-token");
  expect(storage.local).toContainEqual(["console-theme", "dark"]);
});
