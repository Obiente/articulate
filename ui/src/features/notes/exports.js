// Follow-up reminder times use the local `datetime-local` input format.

const pad = (value) => String(value).padStart(2, "0");

// Tomorrow at 9:00 local time, as "YYYY-MM-DDTHH:MM".
export function defaultReminder(now = new Date()) {
  const next = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate() + 1,
    9,
  );
  return `${next.getFullYear()}-${pad(next.getMonth() + 1)}-${pad(next.getDate())}T09:00`;
}

// Unix milliseconds for a local "YYYY-MM-DDTHH:MM", or null.
export function reminderTime(value) {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(value || "");
  if (!match) return null;
  const time = new Date(
    +match[1],
    +match[2] - 1,
    +match[3],
    +match[4],
    +match[5],
  ).getTime();
  return Number.isFinite(time) ? time : null;
}

export function reminderProblem(value, now = Date.now()) {
  const time = reminderTime(value);
  if (time === null) return "Choose a date and time.";
  if (time <= now) return "Choose a time in the future.";
  return null;
}
