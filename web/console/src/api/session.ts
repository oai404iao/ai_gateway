import { clearSession, setSession } from "@/api/session-store";
import type {
  ActivateInvitationInput,
  CompletePasswordResetInput,
  LoginInput,
  LoginResponse,
  RegisterInput,
} from "@/api/types";
import { ApiError, readApiError } from "@/api/errors";
import { apiSend, consoleFetch } from "@/api/client";

const LOGIN_PATH = "/console/v1/auth/login";
const REGISTER_PATH = "/console/v1/auth/register";
const ACTIVATE_PATH = "/console/v1/auth/activate-invitation";

function applyLogin(data: LoginResponse): LoginResponse {
  setSession({
    status: "authenticated",
    accessToken: data.access_token,
    user: data.user,
  });
  return data;
}

export async function login(input: LoginInput): Promise<LoginResponse> {
  // Unauthenticated endpoint; uses credentials:"include" so the response
  // Set-Cookie refresh token is stored. Never auto-refresh on login 401.
  const response = await fetch(LOGIN_PATH, {
    method: "POST",
    credentials: "include",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw await readApiError(response);
  return applyLogin((await response.json()) as LoginResponse);
}

export async function registerAccount(input: RegisterInput): Promise<LoginResponse> {
  const response = await fetch(REGISTER_PATH, {
    method: "POST",
    credentials: "include",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw await readApiError(response);
  return applyLogin((await response.json()) as LoginResponse);
}

export async function activateInvitation(
  input: ActivateInvitationInput,
): Promise<LoginResponse> {
  const response = await fetch(ACTIVATE_PATH, {
    method: "POST",
    credentials: "include",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw await readApiError(response);
  return applyLogin((await response.json()) as LoginResponse);
}

export async function completePasswordReset(
  input: CompletePasswordResetInput,
): Promise<LoginResponse> {
  const data = await apiSend<LoginResponse>(
    "/auth/complete-password-reset",
    "POST",
    input,
  );
  return applyLogin(data);
}

export async function logout(): Promise<void> {
  try {
    await consoleFetch("/auth/logout", { method: "POST" });
  } catch (error) {
    // A failed logout still clears the local session; the refresh cookie was
    // either already cleared by the server or will expire on its own.
    if (error instanceof ApiError && !error.isUnauthorized) {
      // Surface unexpected errors to the caller, but always clear locally.
    }
  } finally {
    clearSession();
  }
}
