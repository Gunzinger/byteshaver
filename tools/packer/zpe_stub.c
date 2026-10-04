/* zpe v3 — Windows self-extracting stub (extract-to-temp + CreateProcess).
 *
 * Container (magic "ZPK3pe64", produced by `zpack.py --pe`):
 *   [stub PE][zstd frame 0][frame 1]...[frame n-1][trailer]
 *   trailer: n x {comp u64, uncomp u64, dest u64}
 *            | nframes u64 | payload_off u64 | magic 8B
 *   dest = offset of the decompressed frame inside the reconstructed
 *   payload file (frames tile the original PE 1:1, including headers,
 *   section alignment padding and any trailing overlay).
 *
 * The stub reads its own file, decompresses the frames at their dest
 * offsets into one flat buffer (the original payload exe bytes) — one
 * thread per frame, dest regions are disjoint — writes it to %TEMP%,
 * CreateProcessA's it with the original command line and environment,
 * waits, propagates the exit code, then deletes the temp file. The child
 * inherits our std handles so its output reaches pipes and redirects
 * (CLI usage, WSL interop).
 *
 * Why extract+CreateProcess instead of in-memory mapping: a manually mapped
 * PE image does not receive OS loader services (native-TLS setup, loader
 * callbacks, SEH interplay, module-list registration) — the v0.7.0
 * experiment crashed on real Windows exactly there (see docs/plans/17 §B,
 * docs/zstd-packer-analysis.md §10.3). The extracted exe is a plain PE that
 * the standard Windows loader handles completely.
 *
 * Env: ZPE_DEBUG=1      -> append phases to %TEMP%\zpe-debug.log
 *                           (also suppresses the error MessageBox)
 *      ZPE_KEEP_TEMP=1  -> keep the extracted exe for inspection
 *      ZPE_THREADS=1    -> decompress serially (benchmarking)
 *
 * Build: see tools/packer/README.md (mingw-gcc or llvm-mingw, static zstd).
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <zstd.h>

static const char MAGIC[8] = {'Z','P','K','3','p','e','6','4'};

/* ---------- debug log (ZPE_DEBUG=1) ---------- */
static HANDLE g_log = NULL;
static int g_debug = 0;
static LARGE_INTEGER g_qpcfreq, g_qpcstart;
static void logline(const char *msg) {
    if (!g_log) return;
    DWORD n; WriteFile(g_log, msg, (DWORD)strlen(msg), &n, NULL);
}
/* milestone with ms offset from process start */
static void phase(const char *msg) {
    char buf[MAX_PATH + 32];
    LARGE_INTEGER now;
    if (!g_log) return;
    QueryPerformanceCounter(&now);
    wsprintfA(buf, "zpe: +%lu ms %s",
              (unsigned long)((now.QuadPart - g_qpcstart.QuadPart) * 1000 / g_qpcfreq.QuadPart),
              msg);
    logline(buf);
}
static void debug_init(void) {
    g_debug = getenv("ZPE_DEBUG") != NULL;
    QueryPerformanceFrequency(&g_qpcfreq);
    QueryPerformanceCounter(&g_qpcstart);
    if (!g_debug) return;
    char dir[MAX_PATH]; UINT n = GetTempPathA(MAX_PATH, dir);
    if (n == 0 || n >= MAX_PATH - 32) return;
    char path[MAX_PATH];
    lstrcpyA(path, dir); lstrcatA(path, "zpe-debug.log");
    g_log = CreateFileA(path, FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE,
                        NULL, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
}

static void die(const char *msg) {
    logline(msg);
    /* GUI-subsystem processes have no stderr — surface fatal errors.
     * Suppressed under ZPE_DEBUG so automation stays headless. */
    if (!g_debug)
        MessageBoxA(NULL, msg, "byteshaver (packed)", MB_ICONERROR | MB_SETFOREGROUND);
    ExitProcess(127);
}

/* ---------- self file ---------- */
static unsigned char *g_self; static size_t g_self_len;

static void read_self(void) {
    char path[MAX_PATH];
    UINT n = GetModuleFileNameA(NULL, path, MAX_PATH);
    if (n == 0 || n >= MAX_PATH) die("zpe: GetModuleFileName failed\n");
    HANDLE h = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
    if (h == INVALID_HANDLE_VALUE) die("zpe: CreateFile(self) failed\n");
    LARGE_INTEGER sz;
    if (!GetFileSizeEx(h, &sz) || sz.QuadPart < 64) die("zpe: file too small\n");
    g_self_len = (size_t)sz.QuadPart;
    g_self = (unsigned char *)HeapAlloc(GetProcessHeap(), 0, g_self_len);
    if (!g_self) die("zpe: OOM (self)\n");
    DWORD got, total = 0;
    while (total < g_self_len && ReadFile(h, g_self + total, (DWORD)(g_self_len - total), &got, NULL) && got)
        total += got;
    CloseHandle(h);
    if (total != g_self_len) die("zpe: short read\n");
    phase("self read\n");
}

/* ---------- decompression (frames -> dest offsets, parallel) ---------- */
static unsigned char *g_payload; static size_t g_payload_len;

typedef struct {
    const unsigned char *src; size_t comp, uncomp; unsigned char *dst;
} frame_job;

static DWORD WINAPI frame_worker(LPVOID param) {
    frame_job *j = (frame_job *)param;
    size_t r = ZSTD_decompress(j->dst, j->uncomp, j->src, j->comp);
    if (ZSTD_isError(r) || r != j->uncomp) die("zpe: decompress failed\n");
    return 0;
}

static void decompress_payload(uint64_t nframes, const uint64_t *tbl, uint64_t payload_off) {
    size_t total = 0;
    for (uint64_t i = 0; i < nframes; i++) {
        uint64_t end = tbl[3*i + 2] + tbl[3*i + 1];
        if (end > total) total = (size_t)end;
    }
    g_payload_len = total;
    g_payload = (unsigned char *)HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, total);
    if (!g_payload) die("zpe: OOM (payload)\n");

    int serial = getenv("ZPE_THREADS") != NULL && atoi(getenv("ZPE_THREADS")) == 1;
    frame_job *jobs = (frame_job *)HeapAlloc(GetProcessHeap(), 0, (SIZE_T)(nframes * sizeof(frame_job)));
    HANDLE *th = (HANDLE *)HeapAlloc(GetProcessHeap(), 0, (SIZE_T)(nframes * sizeof(HANDLE)));
    if (!jobs || !th) die("zpe: OOM (jobs)\n");

    uint64_t src = payload_off; DWORD nthread = 0;
    for (uint64_t i = 0; i < nframes; i++) {
        size_t dest = (size_t)tbl[3*i + 2], uncomp = (size_t)tbl[3*i + 1];
        if (dest > g_payload_len || uncomp > g_payload_len - dest)
            die("zpe: bad frame dest\n");
        jobs[i].src = g_self + src;
        jobs[i].comp = (size_t)tbl[3*i];
        jobs[i].uncomp = uncomp;
        jobs[i].dst = g_payload + dest;
        src += jobs[i].comp;
        if (serial || (th[nthread] = CreateThread(NULL, 0, frame_worker, &jobs[i], 0, NULL)) == NULL)
            frame_worker(&jobs[i]);     /* ZPE_THREADS=1 or CreateThread failed */
        else
            nthread++;
    }
    for (DWORD base = 0; base < nthread; base += MAXIMUM_WAIT_OBJECTS) {
        DWORD n = nthread - base;
        if (n > MAXIMUM_WAIT_OBJECTS) n = MAXIMUM_WAIT_OBJECTS;
        WaitForMultipleObjects(n, th + base, TRUE, INFINITE);
    }
    for (DWORD i = 0; i < nthread; i++) CloseHandle(th[i]);
    HeapFree(GetProcessHeap(), 0, th);
    HeapFree(GetProcessHeap(), 0, jobs);
    phase("decompressed\n");
}

/* ---------- temp exe ---------- */
static char g_temp[MAX_PATH];

static void write_temp_exe(void) {
    char dir[MAX_PATH]; UINT n = GetTempPathA(MAX_PATH, dir);
    if (n == 0 || n >= MAX_PATH - 48) die("zpe: GetTempPath failed\n");
    wsprintfA(g_temp, "%szpe-%lu.exe", dir, (unsigned long)GetCurrentProcessId());
    HANDLE h = CreateFileA(g_temp, GENERIC_WRITE, 0, NULL,
                           CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (h == INVALID_HANDLE_VALUE) die("zpe: CreateFile(temp) failed\n");
    DWORD written, total = 0;
    while (total < g_payload_len && WriteFile(h, g_payload + total, (DWORD)(g_payload_len - total), &written, NULL) && written)
        total += written;
    CloseHandle(h);
    if (total != g_payload_len) die("zpe: short write\n");
    if (g_log) {
        char msg[MAX_PATH + 16];
        wsprintfA(msg, "payload written %s\n", g_temp);
        phase(msg);
    }
}

int main(void) {
    debug_init();
    logline("zpe v3 start\n");
    read_self();

    /* trailer: [nframes u64][payload_off u64][magic 8B] at EOF */
    if (g_self_len < 64 || memcmp(g_self + g_self_len - 8, MAGIC, 8) != 0)
        die("zpe: bad trailer\n");
    uint64_t tail[2];
    memcpy(tail, g_self + g_self_len - 24, 16);
    uint64_t nframes = tail[0], payload_off = tail[1];
    if (nframes == 0 || nframes > 4096 || payload_off + 24 + nframes * 24 > g_self_len)
        die("zpe: bad frame count\n");
    uint64_t *tbl = (uint64_t *)HeapAlloc(GetProcessHeap(), 0, (SIZE_T)(nframes * 24));
    if (!tbl) die("zpe: OOM (table)\n");
    memcpy(tbl, g_self + g_self_len - 24 - nframes * 24, (SIZE_T)(nframes * 24));

    decompress_payload(nframes, tbl, payload_off);
    HeapFree(GetProcessHeap(), 0, tbl);
    HeapFree(GetProcessHeap(), 0, g_self);
    g_self = NULL; g_self_len = 0;

    write_temp_exe();

    /* run the extracted exe; argv[0] stays the packed exe path, args and
     * environment pass through untouched */
    STARTUPINFOA si; PROCESS_INFORMATION pi;
    ZeroMemory(&si, sizeof si); si.cb = sizeof si;
    ZeroMemory(&pi, sizeof pi);
    /* forward our std handles: the child's stdout/stderr must reach our
     * console, pipes and redirects (WSL interop, `packed.exe | tee`) */
    HANDLE out = GetStdHandle(STD_OUTPUT_HANDLE);
    if (out && out != INVALID_HANDLE_VALUE) {
        si.dwFlags = STARTF_USESTDHANDLES;
        si.hStdInput  = GetStdHandle(STD_INPUT_HANDLE);
        si.hStdOutput = out;
        si.hStdError  = GetStdHandle(STD_ERROR_HANDLE);
    }
    /* quote the temp exe so spaces in %TEMP% are safe */
    char cmd[MAX_PATH + 4];
    lstrcpyA(cmd, "\""); lstrcatA(cmd, g_temp); lstrcatA(cmd, "\" ");
    /* append the caller's command line minus argv[0] */
    {
        char *cl = GetCommandLineA();
        /* skip argv[0]: quoted or bare */
        if (*cl == '"') { ++cl; while (*cl && *cl != '"') ++cl; if (*cl) ++cl; }
        else { while (*cl && *cl != ' ' && *cl != '\t') ++cl; while (*cl == ' ' || *cl == '\t') ++cl; }
        lstrcatA(cmd, cl);
    }
    if (!CreateProcessA(g_temp, cmd, NULL, NULL, TRUE, 0, NULL, NULL, &si, &pi))
        die("zpe: CreateProcess failed\n");
    WaitForSingleObject(pi.hProcess, INFINITE);
    DWORD code = 1;
    GetExitCodeProcess(pi.hProcess, &code);
    CloseHandle(pi.hThread); CloseHandle(pi.hProcess);

    if (!getenv("ZPE_KEEP_TEMP")) DeleteFileA(g_temp);
    phase("done\n");
    if (g_log) CloseHandle(g_log);
    ExitProcess(code);
}
