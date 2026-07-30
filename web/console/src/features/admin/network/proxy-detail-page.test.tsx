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
  it("tests an unsaved proxy draft and displays the observed IP metadata", async () => {
    seedAuthenticatedSession();
    let submitted: ProxyTestInput | undefined;
    server.use(
      http.post("/console/v1/network/proxies/test", async ({ request }) => {
        submitted = (await request.json()) as ProxyTestInput;
        return HttpResponse.json(PROXY_TEST_RESULT);
      }),
    );
    const user = userEvent.setup();
    renderAppAt("/admin/network/proxies/new");

    await user.type(
      await screen.findByLabelText(/proxy url/i),
      "http://127.0.0.1:8080",
    );
    await user.type(screen.getByLabelText(/username/i), "proxy-user");
    await user.type(screen.getByLabelText(/password/i), "proxy-password");
    await user.click(screen.getByRole("button", { name: /test proxy/i }));

    expect(await screen.findByText(/proxy test result/i)).toBeInTheDocument();
    expect(screen.getByText(PROXY_TEST_RESULT.ip)).toBeInTheDocument();
    expect(screen.getByText(/los angeles/i)).toBeInTheDocument();
    expect(screen.getByText(PROXY_TEST_RESULT.isp ?? "")).toBeInTheDocument();
    await waitFor(() => {
      expect(submitted).toEqual({
        proxy_url: "http://127.0.0.1:8080",
        username: "proxy-user",
        password: "proxy-password",
      });
    });
  });

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
});
