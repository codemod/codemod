export interface User {
  id: string;
}

export function audit(user: User) {
  return user.id;
}

export class UserRepo {
  save(user: User) {
    audit(user);
  }
}

export interface Store {
  put(key: string): void;
}

export class MemoryStore implements Store {
  put(key: string) {}
}

export class NullStore implements Store {
  put = (key: string) => {};
}
