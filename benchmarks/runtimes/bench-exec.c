/* bench-exec [-t seconds] command [args...]
 *
 * Records BENCH_T0_NS (monotonic CLOCK_UPTIME_RAW on macOS, CLOCK_MONOTONIC
 * elsewhere, in ns) and execs the command in this same process, so the timed
 * span gains no extra process. With -t, an alarm armed before the exec kills
 * the command with SIGALRM (exit 142) after that many seconds. Compiled by
 * lib.sh (ensure_bench_exec) into the git-ignored bin/. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

int main(int argc, char **argv) {
  int i = 1;
  if (argc > 2 && strcmp(argv[1], "-t") == 0) {
    alarm((unsigned)strtoul(argv[2], NULL, 10));
    i = 3;
  }
  if (i >= argc) {
    fprintf(stderr, "usage: bench-exec [-t seconds] command [args...]\n");
    return 64;
  }
  char t0[32];
#ifdef __APPLE__
  unsigned long long ns = clock_gettime_nsec_np(CLOCK_UPTIME_RAW);
#else
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  unsigned long long ns = (unsigned long long)ts.tv_sec * 1000000000ULL + (unsigned long long)ts.tv_nsec;
#endif
  snprintf(t0, sizeof t0, "%llu", ns);
  setenv("BENCH_T0_NS", t0, 1);
  execvp(argv[i], argv + i);
  perror(argv[i]);
  return 127;
}
