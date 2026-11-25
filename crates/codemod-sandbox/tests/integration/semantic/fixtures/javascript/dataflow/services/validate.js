export function validate(email) {
  if (!email.includes("@")) {
    throw new Error("bad email");
  }
}
