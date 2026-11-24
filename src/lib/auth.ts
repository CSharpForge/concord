import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { CurrentUser } from "./user";

// The Discord token never reaches the UI; these calls only return the user.

export const getStatus = () => invoke<CurrentUser | null>("auth_status");
export const startLogin = () => invoke<void>("auth_start_login");
export const logout = () => invoke<void>("auth_logout");

export const onAuthChanged = (cb: (user: CurrentUser | null) => void): Promise<UnlistenFn> =>
  listen<CurrentUser | null>("auth-changed", (e) => cb(e.payload));

export const onLoginFailed = (cb: (message: string) => void): Promise<UnlistenFn> =>
  listen<string>("auth-login-failed", (e) => cb(e.payload));
