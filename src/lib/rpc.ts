import { invoke } from "@tauri-apps/api/core";

export interface ApiError {
  code: string;
  message: string;
  retry_after_secs?: number;
}

let token: string | null = null;
export const setToken = (t: string | null) => {
  token = t;
};

export function isApiError(e: unknown): e is ApiError {
  return typeof e === "object" && e !== null && "code" in e;
}

/** Calls the Host. On the Host device this is Tauri IPC; Phase 5 adds HTTP for Client devices. */
export async function rpc<T = unknown>(method: string, params?: unknown): Promise<T> {
  try {
    return await invoke<T>("rpc", { token, method, params: params ?? null });
  } catch (e) {
    if (isApiError(e)) throw e;
    throw { code: "internal", message: String(e) } satisfies ApiError;
  }
}

export interface SessionUser {
  id: number;
  name: string;
  role: "owner" | "manager" | "cashier" | "kitchen";
  language: Lang;
  theme: "dark" | "light";
  permissions: string[];
}
export type Lang = "fr" | "ar" | "en";
export type Settings = Record<string, string | number | boolean>;
export interface TaxRow {
  id: number;
  name: string;
  rate_bp: number;
  price_include: boolean;
  active: boolean;
}
