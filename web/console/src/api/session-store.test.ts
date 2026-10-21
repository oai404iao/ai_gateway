import { act, renderHook } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { refreshAccessToken } from "@/api/client";
import { login, logout } from "@/api/session";
import { getAccessToken, getSession, setSession } from "@/api/session-store";
import { useSession } from "@/lib/use-session";
import { ADMIN_ACCESS_TOKEN, ADMIN_LOGIN_RESPONSE, ADMIN_USER } from "@/test/fixtures";
import { server } from "@/test/msw";

beforeEach(() => {
  setSession({ status: "loading", accessToken: null, user: null });
});

afterEach(() => vi.restoreAllMocks());

describe("in-memory session state", () => {
  it("updates React consumers and imperative getters without writing browser storage", async () => {
    server.use(http.post("/console/v1/auth/login", () => HttpResponse.json(ADMIN_LOGIN_RESPONSE)));
    const writeStorage = vi.spyOn(Storage.prototype, "setItem");
    const { result } = renderHook(useSession);
    expect(result.current).toEqual({ status: "loading", user: null, isAuthenticated: false });

    await act(async () => {
      await login({ email: ADMIN_USER.email, password: "test-password" });
    });
    expect(result.current).toEqual({
      status: "authenticated", user: ADMIN_USER, isAuthenticated: true,
    });
    expect(getAccessToken()).toBe(ADMIN_ACCESS_TOKEN);

    const updatedUser = { ...ADMIN_USER, display_name: "Updated name" };
    act(() => setSession({ user: updatedUser }));
    expect(result.current.user).toEqual(updatedUser);
    expect(getAccessToken()).toBe(ADMIN_ACCESS_TOKEN);

    await act(logout);
    expect(result.current).toEqual({ status: "unauthenticated", user: null, isAuthenticated: false });
    expect(getSession()).toEqual({ status: "unauthenticated", accessToken: null, user: null });
    expect(writeStorage).not.toHaveBeenCalled();
  });

  it("propagates token refresh and clears subscribers when refresh fails", async () => {
    const { result } = renderHook(useSession);
    await act(async () => {
      expect(await refreshAccessToken()).toBe(ADMIN_ACCESS_TOKEN);
    });
    expect(result.current.isAuthenticated).toBe(true);
    expect(result.current.user).toEqual(ADMIN_USER);

    server.use(http.post("/console/v1/auth/refresh", () => new HttpResponse(null, { status: 401 })));
    await act(async () => {
      expect(await refreshAccessToken()).toBeNull();
    });
    expect(result.current.status).toBe("unauthenticated");
    expect(result.current.user).toBeNull();
    expect(getAccessToken()).toBeNull();
  });
});
