export default function WelcomeEmail({ name }: { name: string }) {
  return <p>Welcome, {name}</p>;
}

export function UnusedEmail() {
  return <p>Never sent</p>;
}
