export const ping = (): string => oldApi("ping");

// Already on the new API, in its final form.
export const pong = (): string => newApi({ name: "pong" });
