/* zpe v3 — Windows self-extracting stub (extract-to-temp + CreateProcess).
 *
 * Container (magic "ZPK3pe64", produced by `zpack.py --pe`):
 *   [stub PE][zstd frame 0][frame 1]...[frame n-1][trailer]
 *   trailer: n x {comp u64, uncomp u64} | nframes u64 | payload_off u64 | magic 8B
 *
 * The stub reads its own file, decompresses the frames into one flat buffer
 * (the original payload exe bytes), writes it to %TEMP%, CreateProcessA's it
 * with the original command line and environment, waits, propagates the exit
 * code, then deletes the temp file.
 *
 * Why extract+CreateProcess instead of in-memory mapping: a manually mapped
 * PE image does not receive OS loader services (native-TLS setup, loader
 * callbacks, SEH interplay, module-list registration) — the v0.7.0
 * experiment crashed on real Windows exactly there (see docs/plans/17 §B,
 * docs/zstd-packer-analysis.md §10.3). The extracted exe is a plain PE that
 * the standard Windows loader handles completely.
 *
 * Env: ZPE_DEBUG=1 -> append phases to %TEMP%\zpe-debug.log
 *
 * Build: see tools/packer/README.md (mingw-gcc or llvm-mingw, static zstd).
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdint.h>
#include <string.h>
#include <zstd.h>

static const char MAGIC[8] = {'Z','P','K','3','p','e','6','4'};

static HANDLE g_log = NULL;
static void logline(const char *msg) {
    if (!g_log) return;
    DWORD n; WriteFile(g_log, msg, (DWORD)strlen(msg), &n, NULL);
}
static void die(const char *msg) {
    logline(msg);
    /* GUI-subsystem processes have no stderr — surface fatal errors */
    MessageBoxA(NULL, msg, "byteshaver (packed)", MB_ICONERROR | MB_SETFOREGROUND);
    ExitProcess(127);
}

/* ---------- debug log (ZPE_DEBUG=1) ---------- */
static void debug_init(void) {
    if (!getenv("ZPE_DEBUG")) return;
    char dir[MAX_PATH]; UINT n = GetTempPathA(MAX_PATH, dir);
    if (n == 0 || n >= MAX_PATH - 32) return;
    char path[MAX_PATH];
    lstrcpyA(path, dir); lstrcatA(path, "zpe-debug.log");
    g_log = CreateFileA(path, FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE,
                        NULL, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
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
    logline("zpe: self read\n");
}

/* ---------- decompression (sequential frames -> one flat buffer) ---------- */
static unsigned char *g_payload; static size_t g_payload_len;

static void decompress_payload(uint64_t nframes, const uint64_t *tbl, uint64_t payload_off) {
    size_t total = 0;
    for (uint64_t i = 0; i < nframes; i++) total += tbl[2*i + 1];
    g_payload_len = total;
    g_payload = (unsigned char *)HeapAlloc(GetProcessHeap(), 0, total);
    if (!g_payload) die("zpe: OOM (payload)\n");
    uint64_t src = payload_off; size_t doff = 0;
    for (uint64_t i = 0; i < nframes; i++) {
        size_t comp = (size_t)tbl[2*i], uncomp = (size_t)tbl[2*i + 1];
        size_t r = ZSTD_decompress(g_payload + doff, uncomp, g_self + src, comp);
        if (ZSTD_isError(r) || r != uncomp) die("zpe: decompress failed\n");
        src += comp; doff += uncomp;
    }
    logline("zpe: decompressed\n");
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
    logline("zpe: payload written\n");
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
    if (nframes == 0 || nframes > 4096 || payload_off + 24 + nframes * 16 > g_self_len)
        die("zpe: bad frame count\n");
    uint64_t *tbl = (uint64_t *)HeapAlloc(GetProcessHeap(), 0, (SIZE_T)(nframes * 16));
    if (!tbl) die("zpe: OOM (table)\n");
    memcpy(tbl, g_self + g_self_len - 24 - nframes * 16, (SIZE_T)(nframes * 16));

    decompress_payload(payload_off, tbl, nframes);
    HeapFree(GetProcessHeap(), 0, tbl);
    HeapFree(GetProcessHeap(), 0, g_self);
    g_self = NULL; g_self_len = 0;

    write_temp_exe();

    /* run the extracted exe; argv[0] stays the packed exe path, args and
     * environment pass through untouched */
    STARTUPINFOA si; PROCESS_INFORMATION pi;
    ZeroMemory(&si, sizeof si); si.cb = sizeof si;
    ZeroMemory(&pi, sizeof pi);
    char exe[MAX_PATH];
    UINT en = GetModuleFileNameA(NULL, exe, MAX_PATH);
    if (en == 0 || en >= MAX_PATH) die("zpe: GetModuleFileName failed\n");
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
    if (!CreateProcessA(g_temp, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi))
        die("zpe: CreateProcess failed\n");
    WaitForSingleObject(pi.hProcess, INFINITE);
    DWORD code = 1;
    GetExitCodeProcess(pi.hProcess, &code);
    CloseHandle(pi.hThread); CloseHandle(pi.hProcess);

    DeleteFileA(g_temp);
    logline("zpe: done\n");
    if (g_log) CloseHandle(g_log);
    ExitProcess(code);
}
