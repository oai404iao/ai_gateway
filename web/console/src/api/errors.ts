import type { ErrorBody } from "@/api/types";

/** A typed Console API failure carrying the HTTP status and server message. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message?: string) {
    super(message ?? code);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }

  /** True for 409 optimistic-concurrency conflicts and 412-style rejections. */
  get isConflict(): boolean {
    return this.status === 409;
  }

  get isUnauthorized(): boolean {
    return this.status === 401;
  }

  get isForbidden(): boolean {
    return this.status === 403;
  }

  get isNotFound(): boolean {
    return this.status === 404;
  }

  get isValidation(): boolean {
    return this.status === 422;
  }
}

export async function readApiError(response: Response): Promise<ApiError> {
  let code = response.statusText || `HTTP ${response.status}`;
  try {
    const body = (await response.clone().json()) as ErrorBody;
    if (body && typeof body.error === "string") code = body.error;
  } catch {
    // Non-JSON error body; keep the status text.
  }
  return new ApiError(response.status, code, code);
}

/** Safe, actionable copy for control-plane mutations with known error codes. */
export function controlPlaneMutationErrorMessage(error: unknown, fallback = "Save failed"): string {
  if (error instanceof ApiError && error.code === "routing_dependency_invalid") {
    return "Save blocked: this change would make the routing configuration invalid. Keep an eligible channel and compatible enabled resources, or update dependent rules first.";
  }
  return error instanceof Error ? error.message : fallback;
}

/** A network-level failure (DNS, CORS, offline) with no HTTP status. */
export class NetworkError extends Error {
  readonly cause?: unknown;

  constructor(message: string, cause?: unknown) {
    super(message);
    this.name = "NetworkError";
    this.cause = cause;
  }
}
