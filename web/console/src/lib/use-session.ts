import { useSyncExternalStore } from "react";
import { getSession, subscribe, type SessionStatus } from "@/api/session-store";
import type { ConsoleUser } from "@/api/types";

export interface UseSessionResult {
  status: SessionStatus;
  user: ConsoleUser | null;
  isAuthenticated: boolean;
}

/** React binding for the in-memory session store. */
export function useSession(): UseSessionResult {
  const snapshot = useSyncExternalStore(subscribe, getSession, getSession);
  return {
    status: snapshot.status,
    user: snapshot.user,
    isAuthenticated: snapshot.status === "authenticated",
  };
}
