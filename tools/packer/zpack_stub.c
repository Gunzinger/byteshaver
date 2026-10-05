/* zpack v2 stub — self-extracting multi-frame zstd packer, multithreaded.
 *
 * Packed file layout:
 *   [stub ELF (padded to page size)][frame 0][frame 1]...[frame n-1][trailer]
 * trailer (from end, little-endian):
 *   frame table: nframes × { comp_len u64, uncomp_len u64 }
 *   nframes u64
 *   payload_off u64
 *   magic "ZPK2zstd" (8 bytes)
 *
 * Frames are independent zstd frames covering contiguous, disjoint regions of
 * the unpacked image; the stub decompresses them in parallel (one pthread per
 * frame, capped by ZPK_THREADS env or CPU count) into one memfd, then fexecve()s it.
 *
 * Env knobs (debug/benchmarking):
 *   ZPK_THREADS=n      cap worker threads at n (1 => serial)
 *   ZPK_DEBUG_SLEEP_MS=n  nanosleep before fexecve (for external /proc sampling)
 *
 * Build (musl, static):
 *   gcc -Os -static -s -ffunction-sections -fdata-sections -Wl,--gc-sections \
 *     zpack_stub2.c <zstd decompress objects> -I<zstd>/lib -lpthread -o zpack-stub2
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/mman.h>
#include <zstd.h>

static const char MAGIC[8] = {'Z','P','K','2','z','s','t','d'};

#ifndef MFD_EXEC
#define MFD_EXEC 0x0010U
#endif

static int memfd_exec(void) {
    int fd = memfd_create("zpk", MFD_CLOEXEC | MFD_EXEC);
    if (fd < 0 && errno == EINVAL)
        fd = memfd_create("zpk", MFD_CLOEXEC);   /* kernels < 6.3 */
    return fd;
}

static long ncpu(void) {
    long n = sysconf(_SC_NPROCESSORS_ONLN);
    return n > 0 ? n : 1;
}

typedef struct {
    const unsigned char *src;
    unsigned char *dst;
    size_t comp, uncomp;
    int rc;              /* 0 ok */
} job_t;

static job_t *g_jobs;
static uint64_t g_nframes;
static volatile long g_next;          /* atomic work-queue cursor */
static int g_rc;                      /* sticky failure flag (atomic accessors) */
static long g_pagesize;               /* hoisted: one sysconf, not one per frame */

static void fail_set(void) { __atomic_store_n(&g_rc, 1, __ATOMIC_RELAXED); }
static int fail_get(void) { return __atomic_load_n(&g_rc, __ATOMIC_RELAXED); }

/* process one frame; returns 0 on success */
static int do_frame(job_t *j) {
    size_t r = ZSTD_decompress(j->dst, j->uncomp, j->src, j->comp);
    j->rc = (ZSTD_isError(r) || r != j->uncomp) ? 1 : 0;
    /* release this frame's clean file-backed input pages immediately so the
     * RSS peak is output + in-flight inputs, not output + whole payload */
    if (j->rc == 0) {
        uintptr_t ps = (uintptr_t)g_pagesize;
        uintptr_t a = (uintptr_t)j->src & ~(ps - 1);
        uintptr_t e = ((uintptr_t)j->src + j->comp + ps - 1) & ~(ps - 1);
        madvise((void *)a, e - a, MADV_DONTNEED);
    }
    return j->rc;
}

static void *worker(void *arg) {
    (void)arg;
    for (;;) {
        long i = __atomic_fetch_add(&g_next, 1, __ATOMIC_RELAXED);
        if (i >= (long)g_nframes || fail_get()) return NULL;
        if (do_frame(&g_jobs[i]) != 0) fail_set();
    }
    return NULL;
}

static void die(const char *msg) {
    size_t n = 0; while (msg[n]) n++;
    (void)write(2, "zpk: ", 5); (void)write(2, msg, n); (void)write(2, "\n", 1);
    exit(127);
}

int main(int argc, char **argv, char **envp) {
    (void)argc;
    g_pagesize = sysconf(_SC_PAGESIZE);
    if (g_pagesize <= 0) g_pagesize = 4096;

    int fd = open("/proc/self/exe", O_RDONLY | O_CLOEXEC);
    if (fd < 0) die("open /proc/self/exe");
    off_t fsz = lseek(fd, 0, SEEK_END);
    if (fsz < (off_t)(40 + 24)) die("file too small");

    uint64_t tail[3]; /* nframes | payload_off | magic */
    if (pread(fd, tail, 24, fsz - 24) != 24 || memcmp((char *)tail + 16, MAGIC, 8) != 0) {
        die("bad trailer");
    }
    uint64_t nframes     = tail[0];
    uint64_t payload_off = tail[1];
    if (nframes == 0 || nframes > 4096) die("bad frame count");

    size_t tbl_sz = (size_t)nframes * 16;
    uint64_t *tbl = malloc(tbl_sz);
    if (!tbl || pread(fd, tbl, tbl_sz, fsz - 24 - (off_t)tbl_sz) != (ssize_t)tbl_sz) die("read frame table");

    /* map payload (contiguous frames, page-aligned offset). Validate the
     * table-derived extents first: a corrupted comp_len used to reach mmap
     * with a bogus length and die by SIGBUS while decompressing; now every
     * frame provably lies inside [payload_off, trailer) and uncomp sums
     * cannot wrap (per-frame cap 1 TiB, nframes <= 4096). */
    uint64_t frames_end = (uint64_t)fsz - 24 - (uint64_t)tbl_sz;
    if (payload_off > frames_end) die("bad payload offset");
    size_t payload_len = 0, unpacked_len = 0;
    for (uint64_t i = 0; i < nframes; i++) {
        if (tbl[2*i] > frames_end - payload_off - (uint64_t)payload_len) die("bad frame table");
        if (tbl[2*i+1] > (1ULL << 40)) die("bad frame table");
        payload_len += tbl[2*i]; unpacked_len += tbl[2*i+1];
    }
    const unsigned char *src = mmap(NULL, payload_len, PROT_READ, MAP_PRIVATE, fd, (off_t)payload_off);
    if (src == MAP_FAILED) die("mmap payload");

    int mfd = memfd_exec();
    if (mfd < 0) die("memfd_create");
    if (ftruncate(mfd, (off_t)unpacked_len) != 0) die("ftruncate");
    unsigned char *dst = mmap(NULL, unpacked_len, PROT_READ | PROT_WRITE, MAP_SHARED, mfd, 0);
    if (dst == MAP_FAILED) die("mmap memfd");

    /* parallel decompression via a shared atomic frame queue: any frame count
     * spreads over min(ncpu, nframes) workers; workers also drop their input
     * pages after each frame, so with nframes >> workers the resident input
     * stays at ~workers x chunk instead of the whole payload */
    long maxth = ncpu();
    const char *env = getenv("ZPK_THREADS");
    if (env) { long v = atol(env); if (v >= 1) maxth = v; }
    if (maxth > (long)nframes) maxth = (long)nframes;

    pthread_t *tid = calloc(maxth > 1 ? maxth - 1 : 1, sizeof(pthread_t));
    job_t *jobs = calloc(nframes, sizeof(job_t));
    if (!tid || !jobs) die("alloc");
    g_jobs = jobs; g_nframes = nframes; g_next = 0; g_rc = 0;

    size_t so = 0, doff = 0;
    for (uint64_t i = 0; i < nframes; i++) {
        jobs[i].src = src + so; jobs[i].dst = dst + doff;
        jobs[i].comp = tbl[2*i]; jobs[i].uncomp = tbl[2*i+1];
        so += tbl[2*i]; doff += tbl[2*i+1];
    }

    long spawned = 0;
    for (long t = 0; t < maxth - 1; t++) {
        if (pthread_create(&tid[t], NULL, worker, NULL) != 0) break;
        spawned++;
    }
    worker(NULL);                                    /* main thread pulls from the queue too */
    for (long t = 0; t < spawned; t++) pthread_join(tid[t], NULL);
    if (fail_get()) die("a frame failed");

    if (getenv("ZPK_DEBUG_SLEEP_MS")) {
        long ms = atol(getenv("ZPK_DEBUG_SLEEP_MS"));
        struct timespec ts = { ms / 1000, (ms % 1000) * 1000000L };
        nanosleep(&ts, NULL);
    }

    fexecve(mfd, argv, envp);
    die("fexecve");
}
