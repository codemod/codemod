import { api, getJson, USERS_URL } from "../api/client";

export function listUsers() {
  return api.get("/users");
}

export function renameUser(id: string, name: string) {
  return api.patch(`/users/${id}`, { name });
}

export function createUser(name: string) {
  return fetch(USERS_URL, { method: "POST", body: JSON.stringify({ name }) });
}

export function deleteUser(id: string) {
  return fetch(USERS_URL + "/" + id, { method: "DELETE" });
}

export function loadTeams() {
  // No route answers this.
  return fetch("/api/teams");
}

export function loadAnything(kind: string) {
  return getJson("/api/" + kind);
}
