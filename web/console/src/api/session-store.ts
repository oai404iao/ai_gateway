import { createStore } from "zustand/vanilla";
import type { ConsoleUser } from "@/api/types";

// Bearer tokens are memory-only. The browser manages the HttpOnly refresh cookie.

export type SessionStatus = "loading" | "authenticated" | "unauthenticated";

interface SessionState {
  status: SessionStatus;
  accessToken: string | null;
  user: ConsoleUser | null;
}

export const sessionStore = createStore<SessionState>(() => ({
  status: "loading",
  accessToken: null,
  user: null,
}));

export function getSession(): SessionState {
  return sessionStore.getState();
}

export function sessionScope(user: ConsoleUser | null): string {
  return user ? `${user.id}:${user.role}:${user.password_change_required}` : "signed-out";
}

export function setSession(session: Partial<SessionState>): void {
  sessionStore.setState(session);
}

export function clearSession(): void {
  sessionStore.setState({ status: "unauthenticated", accessToken: null, user: null });
}

export function getAccessToken(): string | null {
  return sessionStore.getState().accessToken;
}
