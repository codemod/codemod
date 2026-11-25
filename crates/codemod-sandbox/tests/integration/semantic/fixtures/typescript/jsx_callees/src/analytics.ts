export const analytics = {
  track(event: string) {
    return fetch("/t", { method: "POST", body: event });
  },
};
