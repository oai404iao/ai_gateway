import { afterEach, describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BrowserRouter } from "react-router";
import { http, HttpResponse } from "msw";
import { AppProviders } from "@/app/providers";
import { AppRouter } from "@/app/router";
import { STORAGE_KEY, setCurrentLocale } from "@/app/i18n";
import type { ProxyTestInput } from "@/api/types";
import { PROXY, PROXY_TEST_RESULT } from "@/test/fixtures";
import { seedAuthenticatedSession, server } from "@/test/msw";

function renderAppAt(path: string) {
  window.history.replaceState({}, "", path);
  render(
    <AppProviders>
      <BrowserRouter>
        <AppRouter />
      </BrowserRouter>
    </AppProviders>,
  );
}

afterEach(() => {
  window.localStorage.removeItem(STORAGE_KEY);
  setCurrentLocale("en-US");
});

describe("Proxy detail page", () => {
  it("identifies an existing proxy so hidden saved credentials can be reused", async () => {
    seedAuthenticatedSession();
    let submitted: ProxyTestInput | undefined;
    server.use(
      http.post("/console/v1/network/proxies/test", async ({ request }) => {
        submitted = (await request.json()) as ProxyTestInput;
        return HttpResponse.json(PROXY_TEST_RESULT);
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/network/proxies/${PROXY.id}`);

    expect(await screen.findByDisplayValue(PROXY.proxy_url)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /test proxy/i }));

    await screen.findByText(PROXY_TEST_RESULT.ip);
    expect(submitted).toEqual({
      proxy_id: PROXY.id,
      proxy_url: PROXY.proxy_url,
    });
  });

  it("preserves unsaved form edits when window focus triggers a detail refetch", async () => {
    seedAuthenticatedSession();
    let getCount = 0;
    server.use(
      http.get(`/console/v1/network/proxies/${PROXY.id}`, () => {
        getCount += 1;
        return HttpResponse.json(
          { ...PROXY, name: getCount === 1 ? PROXY.name : "Server updated name" },
          { headers: { etag: `"proxy-etag-${getCount}"` } },
        );
      }),
    );
    const user = userEvent.setup();
    renderAppAt(`/admin/network/proxies/${PROXY.id}`);

    const nameInput = await screen.findByDisplayValue(PROXY.name);
    await user.clear(nameInput);
    await user.type(nameInput, "Unsaved local proxy");
    expect(nameInput).toHaveValue("Unsaved local proxy");

    window.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("focus"));

    await waitFor(() => expect(getCount).toBeGreaterThanOrEqual(2));
    expect(nameInput).toHaveValue("Unsaved local proxy");
  });
});
