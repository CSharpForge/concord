export interface CurrentUser {
  id: string;
  username: string;
  global_name?: string | null;
  avatar?: string | null;
}

/** Name shown in the UI: display name if set, otherwise the username. */
export function displayName(user: Pick<CurrentUser, "username" | "global_name">): string {
  const g = user.global_name?.trim();
  return g ? g : user.username;
}

/** One letter for the avatar placeholder. */
export function initial(user: Pick<CurrentUser, "username" | "global_name">): string {
  const first = [...displayName(user)][0];
  return first ? first.toUpperCase() : "?";
}
