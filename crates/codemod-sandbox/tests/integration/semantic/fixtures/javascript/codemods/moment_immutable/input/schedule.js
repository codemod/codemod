import moment from "moment";

export function schedule(start, reminders) {
  const due = moment(start);
  due.add(7, "days");
  const label = due.format("LL");

  const end = moment(start).add(1, "week");

  const pinned = moment(start);
  const alias = pinned;
  alias.subtract(1, "day");

  const later = moment(start);
  reminders.forEach(() => later.startOf("day"));

  return { due, label, end, pinned, later };
}
