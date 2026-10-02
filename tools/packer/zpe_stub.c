/* zpe — Windows PE self-extracting packer stub (x86-64, PE32+).
 *
 * Packed layout (identical to the Linux zpack v2):
 *   [stub PE][zstd frame 0]...[frame n-1][trailer]
 *   trailer: n x {comp u64, uncomp u64} | nframes u64 | payload_off u64 | magic "ZPK2zstd"
 *
 * The stub reads its own file, decompresses the frames in parallel into one
 * buffer, then loads the payload PE entirely in memory:
 *   headers + sections -> base relocation (.reloc) -> imports (LoadLibraryA /
 *   GetProcAddress) -> RtlAddFunctionTable (.pdata) -> per-section protections
 *   -> CreateThread(AddressOfEntryPoint) -> propagate exit code.
 *
 * Build (mingw-w64, static; see tools/packer/README.md):
 *   x86_64-w64-mingw32-gcc -Os -static -s -Wl,--gc-sections -ffunction-sections \
 *     zpe_stub.c libzstd.a -lzstd -o zpe-stub.exe -lpthread
 *
 * Tested under wine 10; see docs/zstd-packer-analysis.md §10 for scope notes
 * (TLS callbacks and C++ exceptions of exotic payloads are not wired up).
 */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <zstd.h>

static const char MAGIC[8] = {'Z','P','K','2','z','s','t','d'};

static void die(const char *msg) {
    DWORD n; WriteFile(GetStdHandle(STD_ERROR_HANDLE), msg, (DWORD)strlen(msg), &n, NULL);
    ExitProcess(127);
}

/* ---------- reading our own file ---------- */

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
    g_self = (unsigned char *)VirtualAlloc(NULL, g_self_len, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    if (!g_self) die("zpe: OOM (self)\n");
    DWORD got, total = 0;
    while (total < g_self_len && ReadFile(h, g_self + total, (DWORD)(g_self_len - total), &got, NULL) && got)
        total += got;
    CloseHandle(h);
    if (total != g_self_len) die("zpe: short read\n");
}

/* ---------- parallel decompression (work queue on CreateThread) ---------- */

typedef struct { unsigned char *src, *dst; size_t comp, uncomp; } job_t;
static job_t *g_jobs; static uint64_t g_nframes; static volatile LONG64 g_next; static volatile LONG g_rc;

static DWORD WINAPI worker(LPVOID arg) {
    (void)arg;
    for (;;) {
        int64_t i = g_next;                                    /* peek */
        if (i >= (int64_t)g_nframes || g_rc) return 0;
        if (InterlockedCompareExchange64(&g_next, i + 1, i) != i) continue;  /* claim */
        job_t *j = &g_jobs[i];
        size_t r = ZSTD_decompress(j->dst, j->uncomp, j->src, j->comp);
        if (ZSTD_isError(r) || r != j->uncomp) InterlockedExchange(&g_rc, 1);
    }
}

static unsigned char *decompress_payload(size_t payload_off, uint64_t nframes, uint64_t *tbl,
                                         size_t *out_len) {
    size_t total = 0;
    for (uint64_t i = 0; i < nframes; i++) total += tbl[2*i + 1];
    *out_len = total;
    unsigned char *buf = (unsigned char *)VirtualAlloc(NULL, total, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    if (!buf) die("zpe: OOM (image)\n");

    g_jobs = (job_t *)HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, (SIZE_T)(nframes * sizeof(job_t)));
    if (!g_jobs) die("zpe: OOM (jobs)\n");
    g_nframes = nframes; g_next = 0; g_rc = 0;
    size_t so = 0, doff = 0;
    for (uint64_t i = 0; i < nframes; i++) {
        g_jobs[i].src = g_self + payload_off + so; g_jobs[i].dst = buf + doff;
        g_jobs[i].comp = tbl[2*i]; g_jobs[i].uncomp = tbl[2*i + 1];
        so += tbl[2*i]; doff += tbl[2*i + 1];
    }

    SYSTEM_INFO si; GetSystemInfo(&si);
    DWORD threads = si.dwNumberOfProcessors; if (threads < 1) threads = 1;
    if (threads > (DWORD)nframes) threads = (DWORD)nframes;
    HANDLE tid[64]; DWORD spawned = 0;
    for (DWORD t = 0; t + 1 < threads && t < 63; t++) {
        tid[t] = CreateThread(NULL, 0, worker, NULL, 0, NULL);
        if (!tid[t]) break;
        spawned++;
    }
    worker(NULL);                                  /* main thread pulls too */
    WaitForMultipleObjects(spawned, tid, TRUE, INFINITE);
    if (g_rc) die("zpe: a frame failed\n");
    HeapFree(GetProcessHeap(), 0, g_jobs);
    return buf;
}

/* ---------- in-memory PE loading ---------- */

typedef BOOL (WINAPI *FnRtlAddFunctionTable)(PIMAGE_RUNTIME_FUNCTION_ENTRY, DWORD, DWORD64);

static void load_pe(unsigned char *img) {
    IMAGE_DOS_HEADER *dos = (IMAGE_DOS_HEADER *)img;
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) die("zpe: bad DOS magic\n");
    IMAGE_NT_HEADERS64 *nt = (IMAGE_NT_HEADERS64 *)(img + dos->e_lfanew);
    if (nt->Signature != IMAGE_NT_SIGNATURE || nt->OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC)
        die("zpe: not PE32+\n");
    IMAGE_FILE_HEADER *fh = &nt->FileHeader;
    IMAGE_OPTIONAL_HEADER64 *oh = &nt->OptionalHeader;

    IMAGE_SECTION_HEADER *sec = IMAGE_FIRST_SECTION(nt);

    /* 1. allocate at preferred base; fall back anywhere + relocate */
    unsigned char *base = (unsigned char *)VirtualAlloc((LPVOID)(uintptr_t)oh->ImageBase,
        oh->SizeOfImage, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    int relocated = 0;
    if (!base) {
        base = (unsigned char *)VirtualAlloc(NULL, oh->SizeOfImage, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
        if (!base) die("zpe: VirtualAlloc(image) failed\n");
        relocated = 1;
    }

    /* 2. headers + sections */
    memcpy(base, img, oh->SizeOfHeaders);
    for (DWORD i = 0; i < fh->NumberOfSections; i++) {
        IMAGE_SECTION_HEADER *s = &sec[i];
        if (s->PointerToRawData && s->SizeOfRawData)
            memcpy(base + s->VirtualAddress, img + s->PointerToRawData, s->SizeOfRawData);
        /* VirtualSize > SizeOfRawData parts stay zero from VirtualAlloc */
    }

    /* 3. base relocation */
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

    /* 4. imports */
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
                    IMAGE_IMPORT_BY_NAME *byname = (IMAGE_IMPORT_BY_NAME *)(base + oft->u1.AddressOfData);
                    fn = GetProcAddress(dll, (LPCSTR)byname->Name);
                }
                if (!fn) die("zpe: GetProcAddress failed\n");
                ft->u1.Function = (ULONG64)(uintptr_t)fn;
            }
        }
    }

    /* 5. unwind info (.pdata) so payload exceptions can unwind */
    DWORD prva = oh->DataDirectory[IMAGE_DIRECTORY_ENTRY_EXCEPTION].VirtualAddress;
    DWORD psz  = oh->DataDirectory[IMAGE_DIRECTORY_ENTRY_EXCEPTION].Size;
    if (prva && psz) {
        FnRtlAddFunctionTable addft = (FnRtlAddFunctionTable)(void *)GetProcAddress(
            GetModuleHandleA("kernel32.dll"), "RtlAddFunctionTable");
        if (addft)
            addft((PIMAGE_RUNTIME_FUNCTION_ENTRY)(base + prva),
                  psz / sizeof(IMAGE_RUNTIME_FUNCTION_ENTRY), (DWORD64)(uintptr_t)base);
    }

    /* 6. section protections */
    for (DWORD i = 0; i < fh->NumberOfSections; i++) {
        IMAGE_SECTION_HEADER *s = &sec[i];
        DWORD prot = PAGE_READWRITE, ch = s->Characteristics;
        if (ch & IMAGE_SCN_MEM_EXECUTE)
            prot = (ch & IMAGE_SCN_MEM_WRITE) ? PAGE_EXECUTE_READWRITE :
                   (ch & IMAGE_SCN_MEM_READ)  ? PAGE_EXECUTE_READ : PAGE_EXECUTE;
        else if (ch & IMAGE_SCN_MEM_WRITE) prot = PAGE_READWRITE;
        else if (ch & IMAGE_SCN_MEM_READ)  prot = PAGE_READONLY;
        DWORD old;
        if (!VirtualProtect(base + s->VirtualAddress,
                            (s->Misc.VirtualSize + 0xfff) & ~0xfffu, prot, &old))
            die("zpe: VirtualProtect failed\n");
    }
    FlushInstructionCache(GetCurrentProcess(), base, oh->SizeOfImage);

    /* 7. start at AddressOfEntryPoint on a fresh thread; propagate exit code */
    LPTHREAD_START_ROUTINE ep =
        (LPTHREAD_START_ROUTINE)(base + oh->AddressOfEntryPoint);
    HANDLE th = CreateThread(NULL, (SIZE_T)oh->SizeOfStackReserve, ep, NULL,
                             STACK_SIZE_PARAM_IS_A_RESERVATION, NULL);
    if (!th) die("zpe: CreateThread(entry) failed\n");
    WaitForSingleObject(th, INFINITE);
    DWORD code = 1;
    GetExitCodeThread(th, &code);
    ExitProcess(code);
}

int main(void) {
    read_self();
    size_t fsz = g_self_len;
    uint64_t tail[3];
    if (fsz < 40 + 24 || memcmp(g_self + fsz - 8, MAGIC, 8) != 0) die("zpe: bad trailer\n");
    memcpy(tail, g_self + fsz - 24, 24);
    uint64_t nframes = tail[0], payload_off = tail[1];
    if (nframes == 0 || nframes > 4096) die("zpe: bad frame count\n");
    uint64_t *tbl = (uint64_t *)HeapAlloc(GetProcessHeap(), 0, (SIZE_T)(nframes * 16));
    if (!tbl || payload_off + 24 + nframes * 16 > fsz) die("zpe: bad trailer\n");
    memcpy(tbl, g_self + fsz - 24 - nframes * 16, (SIZE_T)(nframes * 16));

    size_t img_len;
    unsigned char *img = decompress_payload((size_t)payload_off, nframes, tbl, &img_len);
    load_pe(img);
    return 1;
}
