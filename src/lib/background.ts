import { invoke } from "@tauri-apps/api/core";

export interface BackgroundStatus {
  /** True on Windows only; the UI hides the whole section otherwise. */
  supported: boolean;
  /** Is Concord registered to start when the user signs in? */
  autostart: boolean;
}

export const getBackgroundStatus = () => invoke<BackgroundStatus>("background_status");
/** Returns the resulting state (it can differ from the request if Windows refused). */
export const setAutostart = (enabled: boolean) => invoke<boolean>("background_set_autostart", { enabled });
