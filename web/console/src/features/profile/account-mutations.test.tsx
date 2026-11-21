import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { http, HttpResponse } from "msw";
import { expect, it } from "vitest";
import { getSession } from "@/api/session-store";
import { useChangePassword, useUpdateProfile } from "./api";
import { useRevokeSession } from "@/features/sessions/api";
import { ACTIVE_SESSION, ADMIN_ACCESS_TOKEN, ADMIN_PROFILE, USER_USER } from "@/test/fixtures";
import { seedAuthenticatedSession, seedUserSession, server } from "@/test/msw";

it.each(["profile", "password", "current device"] as const)(
  "keeps the new account when the previous account's %s update completes",
  async (operation) => {
    seedAuthenticatedSession();
    const changePrincipal = ({ request }: { request: Request }) => {
      expect(request.headers.get("Authorization")).toBe(`Bearer ${ADMIN_ACCESS_TOKEN}`);
      seedUserSession();
      return operation === "profile"
        ? HttpResponse.json({ ...ADMIN_PROFILE, display_name: "Renamed admin" })
        : new HttpResponse(null, { status: 204 });
    };
    server.use(
      http.patch("/console/v1/me", changePrincipal),
      http.post("/console/v1/me/password", changePrincipal),
      http.delete(`/console/v1/me/sessions/${ACTIVE_SESSION.id}`, changePrincipal),
    );
    const client = new QueryClient();
    const { result } = renderHook(
      () => ({ profile: useUpdateProfile(), password: useChangePassword(), revoke: useRevokeSession() }),
      { wrapper: ({ children }) => <QueryClientProvider client={client}>{children}</QueryClientProvider> },
    );
    await act(async () => {
      switch (operation) {
        case "profile":
          await result.current.profile.mutateAsync({ display_name: "Renamed admin" });
          break;
        case "password":
          await result.current.password.mutateAsync({ current_password: "old-test-password", new_password: "new-test-password" });
          break;
        case "current device":
          await result.current.revoke.mutateAsync({ id: ACTIVE_SESSION.id, isCurrent: true });
      }
    });
    expect(getSession().user).toEqual(USER_USER);
  },
);
