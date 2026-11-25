export async function invite(appId: string, email: string) {
  return fetch("/console/api/apps/" + appId + "/invite", {
    method: "POST",
    body: JSON.stringify({ email }),
  });
}
