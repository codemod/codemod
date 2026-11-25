"use client";

export function RunButton({ appId }: { appId: string }) {
  async function run(inputs: Record<string, string>) {
    const res = await fetch(`/console/api/apps/${appId}/run`, {
      method: "POST",
      body: JSON.stringify({ inputs }),
    });
    return res.json();
  }

  return <button onClick={() => run({ query: "hello" })}>Run</button>;
}
