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
#include <stdio.h>
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
    char buf[2 * MAX_PATH + 64];
    LARGE_INTEGER now;
    if (!g_log) return;
    QueryPerformanceCounter(&now);
    snprintf(buf, sizeof buf, "zpe: +%lu ms %s",
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
static void append_n(char *dst, size_t cap, size_t *len, const char *src, size_t n) {
    while (n-- && *len + 1 < cap) dst[(*len)++] = *src++;
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

/* <cache_dir>\<key>\<basename of the packed exe>[.<pid>.tmp] — the hash goes
 * into the directory so Task Manager shows the real binary name; the tmp
 * carry a per-process suffix so two concurrent first runs never write the
 * same staging file (a shared <key>.tmp let one process truncate mid-write
 * and promote a torn payload into the cache) */
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
    if (tmp) {
        char pid[16];
        snprintf(pid, sizeof pid, ".%lu.tmp", (unsigned long)GetCurrentProcessId());
        append_bounded(out, MAX_PATH, &n, pid);
    }
}

/* decompression (frames -> dest offsets, parallel) */
static unsigned char *g_payload; static size_t g_payload_len;

typedef struct {
    const unsigned char *src; size_t comp, uncomp; unsigned char *dst;
} frame_job;

static frame_job *g_jobs;
static uint64_t g_nframes;
static volatile LONG64 g_next;         /* work-queue cursor (Interlocked) */
static volatile LONG g_failed;         /* sticky failure flag */

/* claim frames from the shared queue until done or a frame failed */
static DWORD WINAPI worker(LPVOID param) {
    (void)param;
    for (;;) {
        if (g_failed) return 0;
        LONG64 i = InterlockedIncrement64(&g_next) - 1;
        if (i >= (LONG64)g_nframes) return 0;
        frame_job *j = &g_jobs[i];
        size_t r = ZSTD_decompress(j->dst, j->uncomp, j->src, j->comp);
        if (ZSTD_isError(r) || r != j->uncomp) {
            InterlockedExchange(&g_failed, 1);
            die("zpe: decompress failed\n");
        }
    }
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

    frame_job *jobs = (frame_job *)HeapAlloc(GetProcessHeap(), 0, (SIZE_T)(nframes * sizeof(frame_job)));
    if (!jobs) die("zpe: OOM (jobs)\n");

    /* frames must tile [payload_off, frames_end) — unvalidated comp lens used to
     * hand the decoder pointers past the self buffer (wild read; crash or
     * garbage depending on what happened to be mapped there). Order matters
     * (audit CR-5): prove the table fits and payload_off <= frames_end
     * BEFORE any subtraction; nframes <= 4096 keeps 24 + nframes*24 from
     * wrapping, so frames_end - src can never wrap below zero afterwards. */
    if (nframes == 0 || nframes > 4096 || g_self_len < 24 + nframes * 24)
        die("zpe: bad frame count\n");
    uint64_t frames_end = g_self_len - 24 - nframes * 24;
    if (payload_off > frames_end) die("zpe: bad payload offset\n");
    uint64_t src = payload_off;
    for (uint64_t i = 0; i < nframes; i++) {
        size_t dest = (size_t)tbl[3*i + 2], uncomp = (size_t)tbl[3*i + 1];
        size_t comp = (size_t)tbl[3*i];
        if (comp > frames_end - src) die("zpe: bad frame table\n");
        if (dest > g_payload_len || uncomp > g_payload_len - dest)
            die("zpe: bad frame dest\n");
        jobs[i].src = g_self + src;
        jobs[i].comp = comp;
        jobs[i].uncomp = uncomp;
        jobs[i].dst = g_payload + dest;
        src += comp;
    }

    /* min(ncpu, nframes) workers pull from a shared queue (ZPE_THREADS caps
     * further, 1 = serial on the main thread) — the old one-thread-per-frame
     * model degenerated for many small frames */
    DWORD maxth = (DWORD)nframes;
    SYSTEM_INFO si; GetSystemInfo(&si);
    if (si.dwNumberOfProcessors && si.dwNumberOfProcessors < maxth)
        maxth = si.dwNumberOfProcessors;
    const char *te = getenv("ZPE_THREADS");
    if (te) {
        long v = atol(te);
        if (v >= 1 && (DWORD)v < maxth) maxth = (DWORD)v;
    }
    HANDLE *th = (HANDLE *)HeapAlloc(GetProcessHeap(), 0,
                                     (SIZE_T)((maxth > 1 ? maxth - 1 : 1) * sizeof(HANDLE)));
    if (!th) die("zpe: OOM (threads)\n");

    g_jobs = jobs; g_nframes = nframes; g_next = 0; g_failed = 0;
    DWORD nthread = 0;
    for (DWORD t = 0; t + 1 < maxth; t++) {
        if ((th[nthread] = CreateThread(NULL, 0, worker, NULL, 0, NULL)) == NULL)
            break;                      /* main thread covers the shortfall */
        nthread++;
    }
    worker(NULL);                       /* main thread pulls from the queue too */
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

/* ---------- SHA-256 (FIPS 180-4), compact public-domain-style ----------
 * Cache integrity (audit M-33): the steady-state launch path used to trust a
 * size-only check, so any same-size replacement of the cached exe executed
 * with the invoking user's privileges. The first cache write now also stores
 * a <name>.sha256 sidecar holding the hash of the reconstructed payload;
 * every later launch re-hashes the cached file and re-extracts on mismatch
 * (or when a pre-sidecar cache generation is found). This raises the bar a
 * lot but is not a signature: an attacker able to rewrite both files still
 * passes. */
typedef struct {
    uint32_t h[8];
    uint64_t bits;               /* total message length in bits */
    unsigned char buf[64];
    size_t buflen;
} sha256_ctx;

static uint32_t ror32(uint32_t x, int r) { return (x >> r) | (x << (32 - r)); }

static void sha256_block(uint32_t h[8], const unsigned char p[64]) {
    static const uint32_t K[64] = {
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,
        0x923f82a4,0xab1c5ed5,0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,
        0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,0xe49b69c1,0xefbe4786,
        0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,
        0x06ca6351,0x14292967,0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,
        0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,0xa2bfe8a1,0xa81a664b,
        0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,
        0x5b9cca4f,0x682e6ff3,0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,
        0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2
    };
    uint32_t w[64];
    for (int i = 0; i < 16; i++)
        w[i] = ((uint32_t)p[4*i] << 24) | ((uint32_t)p[4*i+1] << 16) |
               ((uint32_t)p[4*i+2] << 8) | (uint32_t)p[4*i+3];
    for (int i = 16; i < 64; i++) {
        uint32_t s0 = ror32(w[i-15], 7) ^ ror32(w[i-15], 18) ^ (w[i-15] >> 3);
        uint32_t s1 = ror32(w[i-2], 17) ^ ror32(w[i-2], 19) ^ (w[i-2] >> 10);
        w[i] = w[i-16] + s0 + w[i-7] + s1;
    }
    uint32_t a = h[0], b = h[1], c = h[2], d = h[3];
    uint32_t e = h[4], f = h[5], g = h[6], hh = h[7];
    for (int i = 0; i < 64; i++) {
        uint32_t S1 = ror32(e, 6) ^ ror32(e, 11) ^ ror32(e, 25);
        uint32_t ch = (e & f) ^ (~e & g);
        uint32_t t1 = hh + S1 + ch + K[i] + w[i];
        uint32_t S0 = ror32(a, 2) ^ ror32(a, 13) ^ ror32(a, 22);
        uint32_t mj = (a & b) ^ (a & c) ^ (b & c);
        uint32_t t2 = S0 + mj;
        hh = g; g = f; f = e; e = d + t1;
        d = c; c = b; b = a; a = t1 + t2;
    }
    h[0] += a; h[1] += b; h[2] += c; h[3] += d;
    h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
}

static void sha256_init(sha256_ctx *c) {
    c->h[0] = 0x6a09e667; c->h[1] = 0xbb67ae85; c->h[2] = 0x3c6ef372; c->h[3] = 0xa54ff53a;
    c->h[4] = 0x510e527f; c->h[5] = 0x9b05688c; c->h[6] = 0x1f83d9ab; c->h[7] = 0x5be0cd19;
    c->bits = 0; c->buflen = 0;
}

static void sha256_update(sha256_ctx *c, const void *data, size_t n) {
    const unsigned char *q = (const unsigned char *)data;
    c->bits += (uint64_t)n << 3;
    while (n) {
        size_t take = 64 - c->buflen;
        if (take > n) take = n;
        memcpy(c->buf + c->buflen, q, take);
        c->buflen += take; q += take; n -= take;
        if (c->buflen == 64) { sha256_block(c->h, c->buf); c->buflen = 0; }
    }
}

static void sha256_final(sha256_ctx *c, unsigned char out[32]) {
    uint64_t bits = c->bits;               /* length before padding */
    c->buf[c->buflen++] = 0x80;
    if (c->buflen > 56) {
        memset(c->buf + c->buflen, 0, 64 - c->buflen);
        sha256_block(c->h, c->buf);
        c->buflen = 0;
    }
    memset(c->buf + c->buflen, 0, 56 - c->buflen);
    for (int i = 0; i < 8; i++)
        c->buf[56 + i] = (unsigned char)(bits >> (56 - 8 * i));
    sha256_block(c->h, c->buf);
    for (int i = 0; i < 8; i++) {
        out[4*i]   = (unsigned char)(c->h[i] >> 24);
        out[4*i+1] = (unsigned char)(c->h[i] >> 16);
        out[4*i+2] = (unsigned char)(c->h[i] >> 8);
        out[4*i+3] = (unsigned char)(c->h[i]);
    }
}

static void sha256_hex(const unsigned char d[32], char out[65]) {
    static const char hex[] = "0123456789abcdef";
    for (int i = 0; i < 32; i++) {
        out[2*i]   = hex[d[i] >> 4];
        out[2*i+1] = hex[d[i] & 0xF];
    }
    out[64] = 0;
}

/* hash a file in chunks (cache entries are ~10s of MB); 0 = failure */
static int sha256_file(const char *path, char out[65]) {
    HANDLE h = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ, NULL,
                           OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
    if (h == INVALID_HANDLE_VALUE) return 0;
    sha256_ctx c;
    sha256_init(&c);
    unsigned char buf[16 * 1024];
    DWORD got;
    int ok = 1;
    for (;;) {
        if (!ReadFile(h, buf, sizeof buf, &got, NULL)) { ok = 0; break; }
        if (got == 0) break;
        sha256_update(&c, buf, got);
    }
    CloseHandle(h);
    if (!ok) return 0;
    unsigned char d[32];
    sha256_final(&c, d);
    sha256_hex(d, out);
    return 1;
}

static void sidecar_path(const char *exe, char *out, size_t cap) {
    snprintf(out, cap, "%s.sha256", exe);
}

/* read the "<64 hex>\n" sidecar; 0 = missing/malformed */
static int read_sidecar(const char *exe, char out[65]) {
    char path[2 * MAX_PATH + 64];
    sidecar_path(exe, path, sizeof path);
    HANDLE h = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ, NULL,
                           OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
    if (h == INVALID_HANDLE_VALUE) return 0;
    char buf[80];
    DWORD got = 0;
    BOOL r = ReadFile(h, buf, sizeof buf - 1, &got, NULL);
    CloseHandle(h);
    if (!r) return 0;
    buf[got] = 0;
    for (int i = 0; i < 64; i++) {
        char ch = buf[i];
        if (!((ch >= '0' && ch <= '9') || (ch >= 'a' && ch <= 'f') || (ch >= 'A' && ch <= 'F')))
            return 0;
        out[i] = ch;
    }
    out[64] = 0;
    return 1;
}

/* best-effort sidecar write; a missing sidecar just costs one re-extract */
static void write_sidecar(const char *exe) {
    char path[2 * MAX_PATH + 64];
    sidecar_path(exe, path, sizeof path);
    sha256_ctx c;
    sha256_init(&c);
    sha256_update(&c, g_payload, g_payload_len);
    unsigned char d[32];
    sha256_final(&c, d);
    char hex[65];
    sha256_hex(d, hex);
    HANDLE h = CreateFileA(path, GENERIC_WRITE, 0, NULL,
                           CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (h == INVALID_HANDLE_VALUE) return;
    DWORD n;
    WriteFile(h, hex, 64, &n, NULL);
    WriteFile(h, "\n", 1, &n, NULL);
    CloseHandle(h);
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
    else
        write_sidecar(g_child);            /* integrity sidecar (M-33) */
    if (g_log) {
        char msg[2 * MAX_PATH + 64];
        snprintf(msg, sizeof msg, "payload written %s\n", g_child);
        phase(msg);
    }
}

/* case-insensitive ".tmp" suffix match (staging-file sweep) */
static int tmp_suffix(const char *name) {
    size_t n = lstrlenA(name);
    return n >= 4 && lstrcmpiA(name + n - 4, ".tmp") == 0;
}

/* keep the cache at one generation: drop entries other than keyname, and
 * sweep crash-leftover staging files (*.tmp, any case) from the current
 * generation */
static void cache_evict_others(const char *keyname) {
    char srch[2 * MAX_PATH + 64], path[2 * MAX_PATH + 64], inner[2 * MAX_PATH + 64];
    snprintf(srch, sizeof srch, "%s\\*", g_cache_dir);
    WIN32_FIND_DATAA fd;
    HANDLE f = FindFirstFileA(srch, &fd);
    if (f == INVALID_HANDLE_VALUE) return;
    do {
        if (!lstrcmpiA(fd.cFileName, ".") || !lstrcmpiA(fd.cFileName, "..")) continue;
        snprintf(path, sizeof path, "%s\\%s", g_cache_dir, fd.cFileName);
        if (fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) {
            int keep = lstrcmpiA(fd.cFileName, keyname) == 0;
            /* wipe the generation's files (everything when evicting, only
             * *.tmp when it is the live generation), then remove the
             * directory (RemoveDirectory may fail while an old gen runs) */
            snprintf(srch, sizeof srch, "%s\\*", path);
            WIN32_FIND_DATAA fd2;
            HANDLE f2 = FindFirstFileA(srch, &fd2);
            if (f2 != INVALID_HANDLE_VALUE) {
                do {
                    if (fd2.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) continue;
                    if (keep && !tmp_suffix(fd2.cFileName)) continue;
                    snprintf(inner, sizeof inner, "%s\\%s", path, fd2.cFileName);
                    DeleteFileA(inner);
                } while (FindNextFileA(f2, &fd2));
                FindClose(f2);
            }
            if (!keep) RemoveDirectoryA(path);
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
    DWORD e = *(DWORD *)(hdr + 0x3C);
    if (e < 0x40 || e + 24 + 70 > sizeof hdr) return 0;   /* bound before any deref */
    if (memcmp(hdr + e, "PE\0\0", 4) != 0) return 0;
    DWORD opt = e + 24;
    if (*(USHORT *)(hdr + opt) != 0x20B) return 0;
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
    /* keep the user-visible argv[0] (the packed exe as invoked) verbatim;
     * the loader uses lpApplicationName, argv[0] is conventional only */
    const char *cl = GetCommandLineA();
    const char *p = cl;
    if (*p == '"') { ++p; while (*p && *p != '"') ++p; if (*p) ++p; }
    else { while (*p && *p != ' ' && *p != '\t') ++p; }
    size_t a0 = (size_t)(p - cl);              /* argv[0] token, incl. quotes */
    while (*p == ' ' || *p == '\t') ++p;       /* skip to the first argument */
    if (a0 + strlen(p) + 2 > sizeof cmd) die("zpe: command line too long\n");
    append_n(cmd, sizeof cmd, &n, cl, a0);
    append_bounded(cmd, sizeof cmd, &n, " ");
    append_bounded(cmd, sizeof cmd, &n, p);
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
    /* non-wrapping validation (audit CR-5): self_size >= 64 is proven above
     * and nframes <= 4096 keeps 24 + nframes*24 from wrapping, so proving
     * the table fits FIRST makes every later subtraction (frames_end,
     * frames_end - src in decompress_payload) provably in range. The old
     * single expression "payload_off + 24 + nframes*24 > self_size" wrapped
     * on hostile payload_off values and let garbage through to the decoder. */
    if (nframes == 0 || nframes > 4096) die("zpe: bad frame count\n");
    if (self_size < 24 + nframes * 24) die("zpe: bad frame count\n");
    uint64_t frames_end = self_size - 24 - nframes * 24;   /* table start */
    if (payload_off > frames_end) die("zpe: bad payload offset\n");

    /* expected payload size + per-frame dest/uncomp sanity, straight from the
     * tail copy of the table — also lets the cache-hit path detect a torn or
     * tampered cache entry (size mismatch) before launching it */
    uint64_t expect_len = 0;
    {
        const unsigned char *tb = tail + tail_len - 24 - nframes * 24;
        for (uint64_t i = 0; i < nframes; i++) {
            uint64_t uncomp, dest;
            memcpy(&uncomp, tb + 24*i + 8, 8);
            memcpy(&dest, tb + 24*i + 16, 8);
            if (uncomp > (1ULL << 40) || dest > (1ULL << 40) || dest + uncomp > (1ULL << 41))
                die("zpe: bad frame table\n");
            if (dest + uncomp > expect_len) expect_len = dest + uncomp;
        }
    }

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

    int have_cached = 0;
    if (use_cache && GetFileAttributesA(entry) != INVALID_FILE_ATTRIBUTES) {
        WIN32_FILE_ATTRIBUTE_DATA fa;
        if (GetFileAttributesExA(entry, GetFileExInfoStandard, &fa) &&
            ((uint64_t)fa.nFileSizeHigh << 32 | fa.nFileSizeLow) == expect_len) {
            /* size is necessary but not sufficient (audit M-33): re-hash the
             * cached bytes and compare against the sidecar stored on first
             * write; mismatch (or a pre-sidecar cache generation) deletes
             * the entry and falls through to a full re-extract */
            char want[65], got[65];
            if (read_sidecar(entry, want) && sha256_file(entry, got) &&
                lstrcmpA(want, got) == 0) {
                lstrcpyA(g_child, entry);
                phase("cache hit\n");
                have_cached = 1;
            } else {
                char sc[2 * MAX_PATH + 64];
                sidecar_path(entry, sc, sizeof sc);
                DeleteFileA(entry);
                DeleteFileA(sc);
                phase("cache entry failed integrity check\n");
            }
        } else {
            phase("cache entry wrong size\n");  /* torn entry: re-extract */
        }
    }
    if (!have_cached) {
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
            snprintf(g_child, MAX_PATH, "%szpe-%lu.exe", dir, (unsigned long)GetCurrentProcessId());
            HANDLE f2 = CreateFileA(g_child, GENERIC_WRITE, 0, NULL,
                                    CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
            if (f2 == INVALID_HANDLE_VALUE) die("zpe: CreateFile(temp) failed\n");
            DWORD written, total = 0;
            while (total < g_payload_len && WriteFile(f2, g_payload + total, (DWORD)(g_payload_len - total), &written, NULL) && written)
                total += written;
            CloseHandle(f2);
            if (total != g_payload_len) die("zpe: short write\n");
            if (g_log) {
                char msg[2 * MAX_PATH + 64];
                snprintf(msg, sizeof msg, "payload written %s\n", g_child);
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
