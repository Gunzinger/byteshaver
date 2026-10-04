/* bench-start — measure child process wall-clock latency on Windows.
 *
 *   bench-start.exe [-n runs] [-w warmup] [-v] <exe> [child-args...]
 *
 * Times CreateProcess -> WaitForSingleObject -> process exit with
 * QueryPerformanceCounter, redirecting the child's stdio to NUL so console
 * output costs nothing. Reports min/p50/p90/mean/max in ms; flags any run
 * whose exit code differs from the first run's (crash / die-path detection:
 * a packed stub failing halfway skews the stats badly).
 *
 * Build (llvm-mingw or mingw-gcc):
 *   x86_64-w64-mingw32-clang -O2 -s -o bench-start.exe bench-start.c
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static double qpc_ms(void) {
    static LARGE_INTEGER f;
    LARGE_INTEGER t;
    if (!f.QuadPart) QueryPerformanceFrequency(&f);
    QueryPerformanceCounter(&t);
    return (double)t.QuadPart * 1000.0 / (double)f.QuadPart;
}

static int cmp_dbl(const void *a, const void *b) {
    double x = *(const double *)a, y = *(const double *)b;
    return (x > y) - (x < y);
}

static int launch(const char *exe, char **args, int nargs, DWORD *exit_code) {
    char cmd[32768];
    int n = snprintf(cmd, sizeof cmd, "\"%s\"", exe);
    for (int i = 0; i < nargs && n > 0 && n < (int)sizeof cmd - 4; i++)
        n += snprintf(cmd + n, sizeof cmd - (size_t)n, " %s", args[i]);
    if (n <= 0 || n >= (int)sizeof cmd) { fprintf(stderr, "bench: cmd too long\n"); return 1; }

    HANDLE nul = CreateFileA("NUL", GENERIC_READ | GENERIC_WRITE,
                             FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_EXISTING, 0, NULL);
    if (nul == INVALID_HANDLE_VALUE) { fprintf(stderr, "bench: open NUL failed\n"); return 1; }
    STARTUPINFOA si; PROCESS_INFORMATION pi;
    ZeroMemory(&si, sizeof si); si.cb = sizeof si;
    si.dwFlags = STARTF_USESTDHANDLES;
    si.hStdInput = si.hStdOutput = si.hStdError = nul;
    ZeroMemory(&pi, sizeof pi);
    BOOL ok = CreateProcessA(exe, cmd, NULL, NULL, TRUE,
                             CREATE_NO_WINDOW, NULL, NULL, &si, &pi);
    CloseHandle(nul);
    if (!ok) { fprintf(stderr, "bench: CreateProcess failed (GLE=%lu)\n", GetLastError()); return 1; }
    WaitForSingleObject(pi.hProcess, INFINITE);
    GetExitCodeProcess(pi.hProcess, exit_code);
    CloseHandle(pi.hThread); CloseHandle(pi.hProcess);
    return 0;
}

int main(int argc, char **argv) {
    int runs = 20, warmup = 3, verbose = 0;
    int argi = 1;
    while (argi < argc && argv[argi][0] == '-' && argv[argi][1]) {
        if (!strcmp(argv[argi], "-n") && argi + 1 < argc) runs = atoi(argv[++argi]);
        else if (!strcmp(argv[argi], "-w") && argi + 1 < argc) warmup = atoi(argv[++argi]);
        else if (!strcmp(argv[argi], "-v")) verbose = 1;
        else { fprintf(stderr, "unknown option: %s\n", argv[argi]); return 2; }
        argi++;
    }
    if (argi >= argc || runs < 1) {
        fprintf(stderr, "usage: bench-start [-n runs] [-w warmup] [-v] <exe> [child-args...]\n");
        return 2;
    }
    const char *exe = argv[argi];
    char **cargs = argv + argi + 1;
    int ncargs = argc - argi - 1;

    DWORD code = 0;
    for (int i = 0; i < warmup; i++)
        if (launch(exe, cargs, ncargs, &code)) return 1;

    double *t = (double *)malloc((size_t)runs * sizeof(double));
    if (!t) return 1;
    for (int i = 0; i < runs; i++) {
        double a = qpc_ms();
        if (launch(exe, cargs, ncargs, &code)) return 1;
        t[i] = qpc_ms() - a;
        if (verbose) printf("run %2d: %8.2f ms (exit %lu)\n", i + 1, t[i], (unsigned long)code);
    }
    DWORD last = code;

    qsort(t, (size_t)runs, sizeof(double), cmp_dbl);
    double mean = 0; for (int i = 0; i < runs; i++) mean += t[i]; mean /= runs;
    printf("bench-start: %s", exe);
    for (int i = 0; i < ncargs; i++) printf(" %s", cargs[i]);
    printf("\n  %d runs, %d warmup | min %7.2f | p50 %7.2f | p90 %7.2f | mean %7.2f | max %7.2f ms\n",
           runs, warmup, t[0], t[runs / 2], t[(runs * 9) / 10], mean, t[runs - 1]);
    if (last != 0)
        printf("  WARNING: child exit code %lu (0x%lX) — non-zero: crash or die() path?\n",
               (unsigned long)last, (unsigned long)last);
    free(t);
    return 0;
}
