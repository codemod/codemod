import type { Store, UserRepo } from "./repo";

// Parameters: the values never say what repo and store are, their types do.
export function register(repo: UserRepo, store: Store) {
  repo.save({ id: "é1" });
  store.put("users");
}

// A library member: no workspace function, but the checker knows where it
// is declared.
export function remember(list: string[]) {
  list.push("user");
}
