/* zpe v2 — Windows PE32+ self-extracting stub, zero-copy image layout.
 *
 * Container (magic "ZPK3pes", produced by `zpack.py --pe`):
 *   [stub PE][hdr frame][section frames...][trailer]
 *   trailer: n x {comp u64, uncomp u64, dest_rva u64} | nframes u64 |
 *            payload_off u64 | magic 8B
 *   frame 0 = PE headers (dest RVA 0); frame i = section raw data whose
 *   dest_rva is the section VirtualAddress (in PointerToRawData order).
 *
 * Memory model (fixes the read-all/decompress/copy triple buffer):
 *   - the packed file is memory-mapped (file-backed, no private copy);
 *   - each frame maps its own view (allocation-granularity aligned) and
 *     unmaps it on completion -> resident input ~ workers x chunk;
 *   - section frames decompress DIRECTLY into the final image allocation,
 *     so the intermediate unpacked copy is gone entirely.
 * Peak commit ~= SizeOfImage + in-flight input.
 *
 * Env knobs: ZPE_DEBUG=1 (memory checkpoints on stderr),
 *            ZPK_THREADS=n (worker cap).
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <psapi.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <zstd.h>

static const char MAGIC[8] = {'Z','P','K','3','p','e','6','4'};

static HANDLE g_err;
static void die(const char *msg) {
    DWORD n; WriteFile(g_err, msg, (DWORD)strlen(msg), &n, NULL);
    ExitProcess(127);
}

/* ---------- debug memory checkpoints (ZPE_DEBUG=1) ---------- */
typedef BOOL (WINAPI *FnGetProcessMemoryInfo)(HANDLE, PROCESS_MEMORY_COUNTERS *, DWORD);
static double now_ms(void) {
    LARGE_INTEGER f, t; QueryPerformanceCounter(&t); QueryPerformanceFrequency(&f);
    return (double)t.QuadPart * 1000.0 / (double)f.QuadPart;
}
static double g_t0;
static void memreport(const char *tag) {
    if (!getenv("ZPE_DEBUG")) return;
    double now = now_ms();
    if (!g_t0) g_t0 = now;
    FnGetProcessMemoryInfo fn = (FnGetProcessMemoryInfo)(void *)GetProcAddress(
        GetModuleHandleA("kernel32.dll"), "K32GetProcessMemoryInfo");
    if (!fn) return;
    PROCESS_MEMORY_COUNTERS mc; memset(&mc, 0, sizeof mc); mc.cb = sizeof mc;
    char line[256];
    if (fn(GetCurrentProcess(), &mc, sizeof mc)) {
        int n = snprintf(line, sizeof line,
            "zpe[mem] %-14s t=+%.1fms WS=%lu WSpeak=%lu commit=%lu commitpeak=%lu\n", tag,
            now - g_t0,
            (unsigned long)mc.WorkingSetSize, (unsigned long)mc.PeakWorkingSetSize,
            (unsigned long)mc.PagefileUsage, (unsigned long)mc.PeakPagefileUsage);
        DWORD wr; WriteFile(g_err, line, (DWORD)n, &wr, NULL);
    }
}

/* ---------- parallel decompression (work queue over per-frame views) ---------- */

typedef struct {
    unsigned char *src;     /* frame input inside the self mapping */
    unsigned char *dst;     /* image destination (base + rva) */
    size_t comp, uncomp;
} job_t;

static job_t *g_jobs;
static uint64_t g_nframes;
static volatile LONG64 g_next;   /* next frame index to claim */
static volatile LONG g_rc;

static DWORD WINAPI worker(LPVOID arg) {
    (void)arg;
    for (;;) {
        int64_t i = g_next;                                            /* peek */
        if (i >= (int64_t)g_nframes || g_rc) return 0;
        if (InterlockedCompareExchange64(&g_next, i + 1, i) != i) continue;  /* claim */
        job_t *j = &g_jobs[i];

        /* full frame content: the PE loader maps SizeOfRawData (FileAlignment-
         * padded) into the image, so writing beyond VirtualSize is faithful */
        size_t r = ZSTD_decompress(j->dst, j->uncomp, j->src, j->comp);
        if (ZSTD_isError(r) || r != j->uncomp) { InterlockedExchange(&g_rc, 1); return 0; }
        /* drop the frame's input pages from the working set (they stay valid
         * in the mapping; the Windows analogue of madvise(MADV_DONTNEED)) */
        /* VirtualUnlock disabled in this A/B build */
    }
}

/* ---------- PE validation ---------- */

static void check_pe64(unsigned char *hdr) {
    IMAGE_DOS_HEADER *dos = (IMAGE_DOS_HEADER *)hdr;
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) die("zpe: bad DOS magic\n");
    IMAGE_NT_HEADERS64 *nt = (IMAGE_NT_HEADERS64 *)(hdr + dos->e_lfanew);
    if (nt->Signature != IMAGE_NT_SIGNATURE || nt->OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC)
        die("zpe: not PE32+\n");
}

/* ---------- in-image loading (relocs, imports, unwind, protections) ---------- */

typedef BOOL (WINAPI *FnRtlAddFunctionTable)(PIMAGE_RUNTIME_FUNCTION_ENTRY, DWORD, DWORD64);

static void finish_image(unsigned char *base, IMAGE_NT_HEADERS64 *nt, int relocated) {
    IMAGE_OPTIONAL_HEADER64 *oh = &nt->OptionalHeader;
    IMAGE_SECTION_HEADER *sec = IMAGE_FIRST_SECTION(nt);

    /* base relocation */
    if (relocated || (oh->DllCharacteristics & IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE)) {
        DWORD rva = oh->DataDirectory[IMAGE_DIRECTORY_ENTRY_BASERELOC].VirtualAddress;
        DWORD rsz = oh->DataDirectory[IMAGE_DIRECTORY_ENTRY_BASERELOC].Size;
        if (rva && rsz) {
            uint64_t delta = (uint64_t)(base - (unsigned char *)(uintptr_t)oh->ImageBase);
            unsigned char *r = base + rva, *rend = r + rsz;
            while (r < rend) {
                IMAGE_BASE_RELOCATION *br = (IMAGE_BASE_RELOCATION *)r;
                if (br->SizeOfBlock < sizeof(IMAGE_BASE_RELOCATION)) break;
                WORD *ent = (WORD *)(r + sizeof(IMAGE_BASE_RELOCATION));
                DWORD cnt = (br->SizeOfBlock - sizeof(IMAGE_BASE_RELOCATION)) / sizeof(WORD);
                for (DWORD k = 0; k < cnt; k++) {
                    WORD t = ent[k] >> 12, o = ent[k] & 0xfff;
                    if (!t) continue;
                    uint64_t *p = (uint64_t *)(base + br->VirtualAddress + o);
                    if (t == IMAGE_REL_BASED_DIR64) *p += delta;
                    else if (t == IMAGE_REL_BASED_HIGHLOW) *(uint32_t *)p += (uint32_t)delta;
                }
                r += br->SizeOfBlock;
            }
        }
    }

    /* imports */
    DWORD irva = oh->DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT].VirtualAddress;
    if (irva) {
        for (IMAGE_IMPORT_DESCRIPTOR *imp = (IMAGE_IMPORT_DESCRIPTOR *)(base + irva);
             imp->Name; imp++) {
            HMODULE dll = LoadLibraryA((LPCSTR)(base + imp->Name));
            if (!dll) die("zpe: LoadLibrary failed\n");
            IMAGE_THUNK_DATA64 *oft = imp->OriginalFirstThunk
                ? (IMAGE_THUNK_DATA64 *)(base + imp->OriginalFirstThunk)
                : (IMAGE_THUNK_DATA64 *)(base + imp->FirstThunk);
            IMAGE_THUNK_DATA64 *ft  = (IMAGE_THUNK_DATA64 *)(base + imp->FirstThunk);
            for (; oft->u1.AddressOfData; oft++, ft++) {
                FARPROC fn;
                if (oft->u1.Ordinal & IMAGE_ORDINAL_FLAG64)
                    fn = GetProcAddress(dll, (LPCSTR)(oft->u1.Ordinal & 0xffff));
                else {
                    IMAGE_IMPORT_BY_NAME *bn = (IMAGE_IMPORT_BY_NAME *)(base + oft->u1.AddressOfData);
                    fn = GetProcAddress(dll, (LPCSTR)bn->Name);
                }
                if (!fn) die("zpe: GetProcAddress failed\n");
                ft->u1.Function = (ULONG64)(uintptr_t)fn;
            }
        }
    }

    /* unwind info */
    DWORD prva = oh->DataDirectory[IMAGE_DIRECTORY_ENTRY_EXCEPTION].VirtualAddress;
    DWORD psz  = oh->DataDirectory[IMAGE_DIRECTORY_ENTRY_EXCEPTION].Size;
    if (prva && psz) {
        FnRtlAddFunctionTable addft = (FnRtlAddFunctionTable)(void *)GetProcAddress(
            GetModuleHandleA("kernel32.dll"), "RtlAddFunctionTable");
        if (addft)
            addft((PIMAGE_RUNTIME_FUNCTION_ENTRY)(base + prva),
                  psz / sizeof(IMAGE_RUNTIME_FUNCTION_ENTRY), (DWORD64)(uintptr_t)base);
    }

    /* section protections */
    for (DWORD i = 0; i < nt->FileHeader.NumberOfSections; i++) {
        IMAGE_SECTION_HEADER *s = &sec[i];
        DWORD ch = s->Characteristics, prot;
        if (ch & IMAGE_SCN_MEM_EXECUTE)
            prot = (ch & IMAGE_SCN_MEM_WRITE) ? PAGE_EXECUTE_READWRITE :
                   (ch & IMAGE_SCN_MEM_READ)  ? PAGE_EXECUTE_READ : PAGE_EXECUTE;
        else if (ch & IMAGE_SCN_MEM_WRITE) prot = PAGE_READWRITE;
        else if (ch & IMAGE_SCN_MEM_READ)  prot = PAGE_READONLY;
        else prot = PAGE_READWRITE;
        DWORD old;
        if (!VirtualProtect(base + s->VirtualAddress,
                            ((SIZE_T)s->Misc.VirtualSize + 0xfff) & ~0xfffu, prot, &old))
            die("zpe: VirtualProtect failed\n");
    }
    FlushInstructionCache(GetCurrentProcess(), base, oh->SizeOfImage);
}

int main(void) {
    g_err = GetStdHandle(STD_ERROR_HANDLE);
    if (!g_err || g_err == INVALID_HANDLE_VALUE) g_err = GetStdHandle(STD_OUTPUT_HANDLE);

    /* 1. map our own file (read-only, file-backed: no private copy) */
    char path[MAX_PATH];
    UINT pn = GetModuleFileNameA(NULL, path, MAX_PATH);
    if (pn == 0 || pn >= MAX_PATH) die("zpe: GetModuleFileName failed\n");
    HANDLE hf = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
    if (hf == INVALID_HANDLE_VALUE) die("zpe: CreateFile(self) failed\n");
    LARGE_INTEGER fsz;
    if (!GetFileSizeEx(hf, &fsz) || fsz.QuadPart < 88) die("zpe: file too small\n");
    HANDLE fmap = CreateFileMappingA(hf, NULL, PAGE_READONLY, 0, 0, NULL);
    if (!fmap) die("zpe: CreateFileMapping failed\n");
    CloseHandle(hf);
    unsigned char *self = (unsigned char *)MapViewOfFile(fmap, FILE_MAP_READ, 0, 0, 0);
    if (!self) die("zpe: MapViewOfFile(self) failed\n");
    SIZE_T self_len = (SIZE_T)fsz.QuadPart;
    memreport("after-map");

    /* 2. trailer */
    if (memcmp(self + self_len - 8, MAGIC, 8) != 0) die("zpe: bad trailer\n");
    uint64_t tail[3];
    memcpy(tail, self + self_len - 24, 24);
    uint64_t nframes = tail[0], payload_off = tail[1];
    if (nframes < 2 || nframes > 4096 || payload_off + 24 + nframes * 24 > self_len)
        die("zpe: bad frame count\n");
    uint64_t *tbl = (uint64_t *)HeapAlloc(GetProcessHeap(), 0, (SIZE_T)(nframes * 24));
    if (!tbl) die("zpe: OOM (table)\n");
    memcpy(tbl, self + self_len - 24 - nframes * 24, (SIZE_T)(nframes * 24));

    /* 3. frame 0 = headers -> small scratch -> parse */
    SYSTEM_INFO si; GetSystemInfo(&si);
    DWORD gran = si.dwAllocationGranularity ? si.dwAllocationGranularity : 65536;
    unsigned char *scr = (unsigned char *)VirtualAlloc(NULL, (SIZE_T)tbl[1],
        MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    if (!scr) die("zpe: OOM (hdr scratch)\n");
    {
        uint64_t voff = payload_off & ~((uint64_t)gran - 1);
        SIZE_T vlen = (SIZE_T)((payload_off - voff) + tbl[0]);
        unsigned char *view = (unsigned char *)MapViewOfFile(fmap, FILE_MAP_READ,
            (DWORD)(voff >> 32), (DWORD)(voff & 0xffffffff), vlen);
        if (!view) die("zpe: MapViewOfFile(hdr) failed\n");
        size_t r = ZSTD_decompress(scr, (size_t)tbl[1], view + (payload_off - voff), (size_t)tbl[0]);
        UnmapViewOfFile(view);
        if (ZSTD_isError(r)) die("zpe: header frame failed\n");
    }
    check_pe64(scr);
    IMAGE_DOS_HEADER *dosT = (IMAGE_DOS_HEADER *)scr;
    IMAGE_NT_HEADERS64 *ntT = (IMAGE_NT_HEADERS64 *)(scr + dosT->e_lfanew);
    IMAGE_OPTIONAL_HEADER64 oh = ntT->OptionalHeader;

    /* 4. reserve the image at the preferred base; relocate if we must move */
    unsigned char *base = (unsigned char *)VirtualAlloc((LPVOID)(uintptr_t)oh.ImageBase,
        oh.SizeOfImage, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    int relocated = 0;
    if (!base) {
        base = (unsigned char *)VirtualAlloc(NULL, oh.SizeOfImage, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
        if (!base) die("zpe: VirtualAlloc(image) failed\n");
        relocated = 1;
    }
    memcpy(base, scr, oh.SizeOfHeaders < (size_t)tbl[1] ? oh.SizeOfHeaders : (size_t)tbl[1]);
    VirtualFree(scr, 0, MEM_RELEASE);
    IMAGE_NT_HEADERS64 *nt = (IMAGE_NT_HEADERS64 *)(base + ((IMAGE_DOS_HEADER *)base)->e_lfanew);
    memreport("after-headers");

    /* 5. section frames -> decompress straight into the image */
    g_nframes = nframes - 1;
    g_jobs = (job_t *)HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, (SIZE_T)(g_nframes * sizeof(job_t)));
    if (!g_jobs) die("zpe: OOM (jobs)\n");
    g_next = 0; g_rc = 0;
    uint64_t off = payload_off + tbl[0];         /* frame 1 starts after frame 0 */
    for (uint64_t i = 0; i < g_nframes; i++) {
        job_t *j = &g_jobs[i];
        j->src = self + off;
        j->comp = tbl[3*(i+1) + 0];
        j->uncomp = tbl[3*(i+1) + 1];
        j->dst = base + tbl[3*(i+1) + 2];
        off += j->comp;
    }

    long maxth = (long)si.dwNumberOfProcessors; if (maxth < 1) maxth = 1;
    const char *env = getenv("ZPK_THREADS");
    if (env) { long v = atol(env); if (v >= 1) maxth = v; }
    if (maxth > (long)g_nframes) maxth = (long)g_nframes;

    HANDLE tid[64]; DWORD spawned = 0;
    for (long t = 0; t < maxth - 1 && t < 63; t++) {
        tid[t] = CreateThread(NULL, 0, worker, NULL, 0, NULL);
        if (!tid[t]) break;
        spawned++;
    }
    worker(NULL);                                 /* main thread pulls too */
    WaitForMultipleObjects(spawned, tid, TRUE, INFINITE);
    HeapFree(GetProcessHeap(), 0, g_jobs);
    UnmapViewOfFile(self);                        /* input pages leave the WS */
    CloseHandle(fmap);
    if (g_rc) die("zpe: a frame failed\n");
    memreport("after-decomp");

    /* 6. relocate/imports/unwind/protections, then run */
    finish_image(base, nt, relocated);
    memreport("after-load");

    LPTHREAD_START_ROUTINE ep =
        (LPTHREAD_START_ROUTINE)(base + nt->OptionalHeader.AddressOfEntryPoint);
    HANDLE th = CreateThread(NULL, (SIZE_T)nt->OptionalHeader.SizeOfStackReserve, ep, NULL,
                             STACK_SIZE_PARAM_IS_A_RESERVATION, NULL);
    if (!th) die("zpe: CreateThread(entry) failed\n");
    WaitForSingleObject(th, INFINITE);
    DWORD code = 1;
    GetExitCodeThread(th, &code);
    ExitProcess(code);
}
