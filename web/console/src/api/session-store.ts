import { createStore } from "zustand/vanilla";
import type { ConsoleUser } from "@/api/types";

// In-memory only. Access tokens, refresh tokens, and API keys are never
// persisted to localStorage, sessionStorage, URLs, or query caches.
// The refresh token lives only in the HttpOnly; Secure; SameSite=Lax cookie
// managed by the browser, so the SPA never reads it.

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

export function setSession(session: Partial<SessionState>): void {
  sessionStore.setState(session);
}

export function clearSession(): void {
  sessionStore.setState({ status: "unauthenticated", accessToken: null, user: null });
}

export function getAccessToken(): string | null {
  return sessionStore.getState().accessToken;
}
