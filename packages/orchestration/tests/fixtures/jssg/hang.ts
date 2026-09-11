// Never returns; used to prove that aborting a run kills the bridge process.
export default async function transform(): Promise<string | null> {
  for (;;) {
    // spin
  }
}
