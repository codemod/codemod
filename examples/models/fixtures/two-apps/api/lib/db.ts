type User = { id: string; name: string };

const users = new Map<string, User>();

export function findUser(id: string) {
  return users.get(id) ?? null;
}

export function allUsers() {
  return [...users.values()];
}

export function insertUser(name: string) {
  const user = { id: String(users.size + 1), name };
  users.set(user.id, user);
  return user;
}

export function updateUser(id: string, name: string) {
  const user = users.get(id);
  if (user) user.name = name;
  return user ?? null;
}

export function removeUser(id: string) {
  return users.delete(id);
}

// Nothing calls this, and no model says anything does.
export function purgeUsers() {
  users.clear();
}
