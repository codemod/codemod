import { formatName } from "./format.js";

export function Profile({ user, theme }) {
  const title = formatName(user.first, user.last);
  const cls = theme === "dark" ? "card dark" : "card";
  return <Card title={title} className={cls} onClose={() => close(user.id)} />;
}
