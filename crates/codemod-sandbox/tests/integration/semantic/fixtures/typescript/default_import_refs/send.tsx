import Welcome from "./emails/WelcomeEmail";

export function sendWelcome(name: string) {
  return render(<Welcome name={name} />);
}
