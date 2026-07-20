import type { ConsoleUser } from "@/api/types";

// In-memory only. Access tokens, refresh tokens, and API keys are never
// persisted to localStorage, sessionStorage, URLs, or query caches.
// The refresh token lives only in the HttpOnly; Secure; SameSite=Lax cookie
// managed by the browser, so the SPA never reads it.

export type SessionStatus = "loading" | "authenticated" | "unauthenticated";

type Listener = () => void;

interface SessionState {
  status: SessionStatus;
  accessToken: string | null;
  user: ConsoleUser | null;
}

let state: SessionState = {
  status: "loading",
  accessToken: null,
  user: null,
};

const listeners = new Set<Listener>();

export function getSession(): SessionState {
  return state;
}

export function setSession(session: Partial<SessionState>): void {
  state = { ...state, ...session };
  for (const listener of listeners) listener();
}

export function clearSession(): void {
  state = { status: "unauthenticated", accessToken: null, user: null };
  for (const listener of listeners) listener();
}

export function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getAccessToken(): string | null {
  return state.accessToken;
}
