import { useStore } from "zustand";
import { sessionStore, type SessionStatus } from "@/api/session-store";
import type { ConsoleUser } from "@/api/types";

export interface UseSessionResult {
  status: SessionStatus;
  user: ConsoleUser | null;
  isAuthenticated: boolean;
}

/** React binding for the in-memory session store. */
export function useSession(): UseSessionResult {
  const status = useStore(sessionStore, (state) => state.status);
  const user = useStore(sessionStore, (state) => state.user);
  return {
    status,
    user,
    isAuthenticated: status === "authenticated",
  };
}
