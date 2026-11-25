import moment from "moment";

export function schedule(start, reminders) {
  let due = moment(start);
  due = due.add(7, "days");
  const label = due.format("LL");

  const end = moment(start).add(1, "week");

  const pinned = moment(start);
  const alias = pinned;
  /* TODO(moment-immutable): mutates `pinned` through an alias */ alias.subtract(1, "day");

  const later = moment(start);
  reminders.forEach(() => /* TODO(moment-immutable): deferred mutation of `later` */ later.startOf("day"));

  return { due, label, end, pinned, later };
}
