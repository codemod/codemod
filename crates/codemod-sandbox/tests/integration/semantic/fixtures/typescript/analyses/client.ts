import { trpc } from "./trpc";

export function Profile() {
  const user = trpc.user.getUser.useQuery();
  return format(user);
}

function format(value: unknown) {
  return String(value);
}
