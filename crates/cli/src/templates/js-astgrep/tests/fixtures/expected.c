// sprintf() calls are rewritten to the bounded snprintf().
#include <stdio.h>

void greet(const char *name) {
    char message[64];
    snprintf(message, sizeof(message), "Hello, %s!", name);
    puts(message);
}

void report(int count, double ratio) {
    char line[32];
    snprintf(line, sizeof(line), "%d items (%.2f)", count, ratio);
    puts(line);

    // Already bounded: left alone.
    char summary[32];
    snprintf(summary, sizeof(summary), "%d items", count);
    puts(summary);
}

void read_name(void) {
    char name[16];
    gets(name);
    greet(name);
}
