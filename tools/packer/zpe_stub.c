/* zpe v3 — Windows self-extracting stub (extract-to-cache + CreateProcess).
 *
 * Container (magic "ZPK3pe64", produced by `zpack.py --pe`):
 *   [stub PE][zstd frame 0][frame 1]...[frame n-1][trailer]
 *   trailer: n x {comp u64, uncomp u64, dest u64}
 *            | nframes u64 | payload_off u64 | magic 8B
 *   dest = offset of the decompressed frame inside the reconstructed
 *   payload file (frames tile the original PE 1:1, including headers,
 *   section alignment padding and any trailing overlay).
 *
 * First run:  read self, decompress the frames at their dest offsets into
 *             one flat buffer (the original payload exe bytes) — one thread
 *             per frame, dest regions are disjoint — and move it into the
 *             cache as %LOCALAPPDATA%\byteshaver\zpe-cache\<key>.exe
 *             (written via <key>.tmp + MoveFileEx so a crash never leaves a
 *             half-written cache entry). Older cache generations are
 *             deleted on miss, so the cache holds one payload.
 * Later runs: the cached exe exists — launch it directly. This skips
 *             decompression, the 16 MB write and the Defender fresh-file
 *             scan, putting steady-state startup close to the unpacked
 *             binary.
 * <key> is an FNV-1a over self size, payload offset, frame-table bytes and
 * the first payload bytes — a repacked payload always remaps to a new key.
 *
 * The cached exe is then CreateProcessA'd with the original command line
 * and environment; the stub waits and propagates the exit code. The child
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
 * Env: ZPE_DEBUG=1      -> append phases (QPC-timed) to %TEMP%\zpe-debug.log
 *                           (also suppresses the error MessageBox)
 *      ZPE_NO_CACHE=1   -> extract to %TEMP% and delete after exit
 *      ZPE_KEEP_TEMP=1  -> with ZPE_NO_CACHE: keep the temp exe
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

/* ---------- small unbounded appends (cmd building) ---------- */
static void append_bounded(char *dst, size_t cap, size_t *len, const char *src) {
    while (*src && *len + 1 < cap) dst[(*len)++] = *src++;
    dst[*len] = 0;
}

/* ---------- self file ---------- */
static char g_self_path[MAX_PATH];
static unsigned char *g_self; static size_t g_self_len;

static UINT self_path(void) {
    UINT n = GetModuleFileNameA(NULL, g_self_path, MAX_PATH);
    if (n == 0 || n >= MAX_PATH) die("zpe: GetModuleFileName failed\n");
    return n;
}
/* read [off, off+len) of our own file; returns bytes read (0 = fail) */
static DWORD pread_range(uint64_t off, void *buf, DWORD len) {
    HANDLE h = CreateFileA(g_self_path, GENERIC_READ, FILE_SHARE_READ, NULL,
                           OPEN_EXISTING, 0, NULL);
    DWORD got = 0;
    if (h == INVALID_HANDLE_VALUE) return 0;
    LARGE_INTEGER li; li.QuadPart = (LONGLONG)off;
    if (SetFilePointerEx(h, li, NULL, FILE_BEGIN) && ReadFile(h, buf, len, &got, NULL))
        ; else got = 0;
    CloseHandle(h);
    return got;
}
static void read_self(void) {
    HANDLE h = CreateFileA(g_self_path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
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

/* ---------- cache ---------- */
static char g_cache_dir[MAX_PATH], g_child[MAX_PATH], g_fallback_tmp[MAX_PATH];

static uint64_t fnv1a(uint64_t h, const void *p, size_t n) {
    const unsigned char *b = (const unsigned char *)p;
    while (n--) { h ^= *b++; h *= 0x100000001b3ULL; }
    return h;
}

/* <cache_dir>\<key>\<basename of the packed exe>[.tmp] — the hash goes into
 * the directory so Task Manager shows the real binary name */
static void cache_paths(const char *keyname, int tmp, char *out) {
    size_t n = 0;
    append_bounded(out, MAX_PATH, &n, g_cache_dir);
    append_bounded(out, MAX_PATH, &n, "\\");
    append_bounded(out, MAX_PATH, &n, keyname);
    CreateDirectoryA(out, NULL);           /* idempotent */
    append_bounded(out, MAX_PATH, &n, "\\");
    const char *base = g_self_path + lstrlenA(g_self_path);
    while (base > g_self_path && base[-1] != '\\' && base[-1] != '/') --base;
    append_bounded(out, MAX_PATH, &n, base);
    if (tmp) append_bounded(out, MAX_PATH, &n, ".tmp");
}

/* decompression (frames -> dest offsets, parallel) */
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

/* write the flat buffer into the cache: <key>.tmp + MoveFileEx, so a crash
 * mid-write never leaves a runnable-looking half payload */
static void write_cache_exe(const char *keyname) {
    cache_paths(keyname, 1, g_fallback_tmp);
    cache_paths(keyname, 0, g_child);
    HANDLE h = CreateFileA(g_fallback_tmp, GENERIC_WRITE, 0, NULL,
                           CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (h == INVALID_HANDLE_VALUE) die("zpe: CreateFile(cache) failed\n");
    DWORD written, total = 0;
    while (total < g_payload_len && WriteFile(h, g_payload + total, (DWORD)(g_payload_len - total), &written, NULL) && written)
        total += written;
    CloseHandle(h);
    if (total != g_payload_len) die("zpe: short write\n");
    if (!MoveFileExA(g_fallback_tmp, g_child, MOVEFILE_REPLACE_EXISTING))
        lstrcpyA(g_child, g_fallback_tmp); /* volatile: launched, never cached */
    if (g_log) {
        char msg[MAX_PATH + 16];
        wsprintfA(msg, "payload written %s\n", g_child);
        phase(msg);
    }
}

/* keep the cache at one generation: drop entries other than keyname */
static void cache_evict_others(const char *keyname) {
    char srch[MAX_PATH], path[MAX_PATH], inner[MAX_PATH];
    wsprintfA(srch, "%s\\*", g_cache_dir);
    WIN32_FIND_DATAA fd;
    HANDLE f = FindFirstFileA(srch, &fd);
    if (f == INVALID_HANDLE_VALUE) return;
    do {
        if (!lstrcmpiA(fd.cFileName, ".") || !lstrcmpiA(fd.cFileName, "..")) continue;
        wsprintfA(path, "%s\\%s", g_cache_dir, fd.cFileName);
        if (fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) {
            if (lstrcmpiA(fd.cFileName, keyname) == 0) continue;
            /* wipe the generation's files, then remove the directory
             * (RemoveDirectory may fail while an old gen still runs) */
            wsprintfA(srch, "%s\\*", path);
            WIN32_FIND_DATAA fd2;
            HANDLE f2 = FindFirstFileA(srch, &fd2);
            if (f2 != INVALID_HANDLE_VALUE) {
                do {
                    if (fd2.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) continue;
                    wsprintfA(inner, "%s\\%s", path, fd2.cFileName);
                    DeleteFileA(inner);
                } while (FindNextFileA(f2, &fd2));
                FindClose(f2);
            }
            RemoveDirectoryA(path);
        } else {
            DeleteFileA(path);      /* legacy flat <hash>.exe layout */
        }
    } while (FindNextFileA(f, &fd));
    FindClose(f);
}

/* our own optional-header Subsystem — zpack.py patches it to the payload's,
 * so the stub knows whether it is wrapping a GUI or console payload */
static USHORT self_subsystem(void) {
    static unsigned char hdr[0x400];
    if (pread_range(0, hdr, sizeof hdr) != sizeof hdr) return 0;
    if (memcmp(hdr + *(DWORD *)(hdr + 0x3C), "PE\0\0", 4) != 0) return 0;
    DWORD opt = *(DWORD *)(hdr + 0x3C) + 24;
    if (opt + 70 > sizeof hdr || *(USHORT *)(hdr + opt) != 0x20B) return 0;
    return *(USHORT *)(hdr + opt + 68);
}

static int g_gui_detach;

/* run the extracted exe; argv[0] stays the packed exe path, args and
 * environment pass through untouched; std handles are forwarded */
static DWORD launch_child(const char *exe) {
    STARTUPINFOA si; PROCESS_INFORMATION pi;
    ZeroMemory(&si, sizeof si); si.cb = sizeof si;
    ZeroMemory(&pi, sizeof pi);
    HANDLE out = GetStdHandle(STD_OUTPUT_HANDLE);
    if (out && out != INVALID_HANDLE_VALUE) {
        si.dwFlags = STARTF_USESTDHANDLES;
        si.hStdInput  = GetStdHandle(STD_INPUT_HANDLE);
        si.hStdOutput = out;
        si.hStdError  = GetStdHandle(STD_ERROR_HANDLE);
    }
    char cmd[32768];
    size_t n = 0;
    append_bounded(cmd, sizeof cmd, &n, "\"");
    append_bounded(cmd, sizeof cmd, &n, exe);
    append_bounded(cmd, sizeof cmd, &n, "\" ");
    {
        char *cl = GetCommandLineA();
        if (*cl == '"') { ++cl; while (*cl && *cl != '"') ++cl; if (*cl) ++cl; }
        else { while (*cl && *cl != ' ' && *cl != '\t') ++cl; while (*cl == ' ' || *cl == '\t') ++cl; }
        append_bounded(cmd, sizeof cmd, &n, cl);
    }
    if (!CreateProcessA(exe, cmd, NULL, NULL, TRUE, 0, NULL, NULL, &si, &pi))
        die("zpe: CreateProcess failed\n");
    if (g_gui_detach) {
        /* fire-and-forget: GUI payloads need no exit code and no console —
         * the stub vanishes so only the app process stays visible */
        CloseHandle(pi.hThread); CloseHandle(pi.hProcess);
        phase("launched (gui detach)\n");
        return 0;
    }
    WaitForSingleObject(pi.hProcess, INFINITE);
    DWORD code = 1;
    GetExitCodeProcess(pi.hProcess, &code);
    CloseHandle(pi.hThread); CloseHandle(pi.hProcess);
    return code;
}

int main(void) {
    debug_init();
    logline("zpe v3 start\n");
    self_path();
    g_gui_detach = self_subsystem() == IMAGE_SUBSYSTEM_WINDOWS_GUI;

    /* trailer + frame table live at EOF; 128 KB covers the 4096-frame max
     * (4096*24 + 24). Small reads only — the full file is loaded on miss. */
    static unsigned char tail[128 * 1024];
    HANDLE h = CreateFileA(g_self_path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
    if (h == INVALID_HANDLE_VALUE) die("zpe: CreateFile(self) failed\n");
    LARGE_INTEGER sz;
    if (!GetFileSizeEx(h, &sz) || sz.QuadPart < 64) die("zpe: file too small\n");
    CloseHandle(h);
    uint64_t self_size = (uint64_t)sz.QuadPart;
    uint64_t tail_off = self_size > sizeof tail ? self_size - sizeof tail : 0;
    DWORD tail_len = pread_range(tail_off, tail, (DWORD)(self_size - tail_off));
    if (tail_len < 64 || tail_len > sizeof tail) die("zpe: tail read failed\n");
    if (memcmp(tail + tail_len - 8, MAGIC, 8) != 0) die("zpe: bad trailer\n");
    uint64_t nframes, payload_off;
    memcpy(&nframes, tail + tail_len - 24, 8);
    memcpy(&payload_off, tail + tail_len - 16, 8);
    if (nframes == 0 || nframes > 4096 || payload_off + 24 + nframes * 24 > self_size)
        die("zpe: bad frame count\n");

    /* cache key: FNV-1a over sizes, frame table and the first payload bytes */
    uint64_t key = 0xcbf29ce484222325ULL;
    key = fnv1a(key, &self_size, sizeof self_size);
    key = fnv1a(key, &payload_off, sizeof payload_off);
    key = fnv1a(key, &nframes, sizeof nframes);
    key = fnv1a(key, tail, tail_len);
    unsigned char head[256];
    DWORD head_got = pread_range(payload_off, head, sizeof head);
    if (head_got) key = fnv1a(key, head, head_got);
    static const char hex[] = "0123456789abcdef";
    char keyname[17];
    for (int i = 0; i < 16; i++) keyname[i] = hex[(key >> (60 - 4 * i)) & 0xF];
    keyname[16] = 0;

    /* cache dir: %LOCALAPPDATA%\byteshaver\zpe-cache (fallback %TEMP%) */
    size_t cd = 0;
    if (!GetEnvironmentVariableA("LOCALAPPDATA", g_cache_dir, MAX_PATH - 40))
        GetTempPathA(MAX_PATH - 40, g_cache_dir);
    cd = lstrlenA(g_cache_dir);
    while (cd && (g_cache_dir[cd - 1] == '\\' || g_cache_dir[cd - 1] == '/'))
        g_cache_dir[--cd] = 0;
    g_cache_dir[cd] = 0;
    append_bounded(g_cache_dir, MAX_PATH, &cd, "\\byteshaver");
    CreateDirectoryA(g_cache_dir, NULL);           /* may already exist */
    append_bounded(g_cache_dir, MAX_PATH, &cd, "\\zpe-cache");
    CreateDirectoryA(g_cache_dir, NULL);

    int use_cache = getenv("ZPE_NO_CACHE") == NULL;
    char entry[MAX_PATH];
    cache_paths(keyname, 0, entry);

    if (use_cache && GetFileAttributesA(entry) != INVALID_FILE_ATTRIBUTES) {
        lstrcpyA(g_child, entry);
        phase("cache hit\n");
    } else {
        read_self(); /* full read; trailer/table already validated above */
        uint64_t *tbl = (uint64_t *)HeapAlloc(GetProcessHeap(), 0, (SIZE_T)(nframes * 24));
        if (!tbl) die("zpe: OOM (table)\n");
        memcpy(tbl, g_self + g_self_len - 24 - nframes * 24, (SIZE_T)(nframes * 24));
        decompress_payload(nframes, tbl, payload_off);
        HeapFree(GetProcessHeap(), 0, tbl);
        HeapFree(GetProcessHeap(), 0, g_self);
        g_self = NULL; g_self_len = 0;

        if (use_cache) {
            write_cache_exe(keyname);
            cache_evict_others(keyname);
        } else {
            /* legacy temp path */
            char dir[MAX_PATH]; UINT n = GetTempPathA(MAX_PATH, dir);
            if (n == 0 || n >= MAX_PATH - 48) die("zpe: GetTempPath failed\n");
            wsprintfA(g_child, "%szpe-%lu.exe", dir, (unsigned long)GetCurrentProcessId());
            HANDLE f2 = CreateFileA(g_child, GENERIC_WRITE, 0, NULL,
                                    CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
            if (f2 == INVALID_HANDLE_VALUE) die("zpe: CreateFile(temp) failed\n");
            DWORD written, total = 0;
            while (total < g_payload_len && WriteFile(f2, g_payload + total, (DWORD)(g_payload_len - total), &written, NULL) && written)
                total += written;
            CloseHandle(f2);
            if (total != g_payload_len) die("zpe: short write\n");
            if (g_log) {
                char msg[MAX_PATH + 16];
                wsprintfA(msg, "payload written %s\n", g_child);
                phase(msg);
            }
        }
    }

    DWORD code = launch_child(g_child);

    /* a detached GUI child still runs from the file — deleting (no-cache
     * mode) would fail anyway */
    if (!g_gui_detach && !use_cache && !getenv("ZPE_KEEP_TEMP")) DeleteFileA(g_child);
    phase("done\n");
    if (g_log) CloseHandle(g_log);
    ExitProcess(code);
}
