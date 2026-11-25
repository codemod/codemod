import axios from "axios";

export const api = axios.create({ baseURL: "/api" });

export const USERS_URL = "/api/users";

// The URL comes from the caller: this model cannot see which route it is.
export async function getJson(path: string) {
  const response = await fetch(path);
  return response.json();
}
