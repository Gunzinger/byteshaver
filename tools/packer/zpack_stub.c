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
 *   ZPK_THREADS=n         cap worker threads at n (1 => serial)
 *   ZPK_DEBUG_SLEEP_MS=n  nanosleep before fexecve (for external /proc sampling)
 *
 * Build (musl, static; see tools/packer/README.md for the full recipe):
 *   gcc -Os -static -s -ffunction-sections -fdata-sections -Wl,--gc-sections \
 *     zpack_stub.c libzstd.a -I<zstd>/lib -lpthread -o zpack-stub
 *   libzstd.a must be built with: ZSTD_LEGACY_SUPPORT=0 CFLAGS+=" -DDYNAMIC_BMI2=0"
 *   (drops legacy-format decoders and the duplicated BMI2 code paths: 194 KB => 92 KB)
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

static void *worker(void *arg) {
    job_t *j = (job_t *)arg;
    size_t r = ZSTD_decompress(j->dst, j->uncomp, j->src, j->comp);
    j->rc = (ZSTD_isError(r) || r != j->uncomp) ? 1 : 0;
    return j;
}

static void werr(const char *msg, size_t n) { (void)write(2, msg, n); }
static void die(const char *msg) {
    size_t n = 0; while (msg[n]) n++;
    (void)write(2, "zpk: ", 5); (void)write(2, msg, n); (void)write(2, "\n", 1);
    exit(127);
}
#include <string.h>

int main(int argc, char **argv, char **envp) {
    int fd = open("/proc/self/exe", O_RDONLY | O_CLOEXEC);
    if (fd < 0) die("zpk: open /proc/self/exe");
    off_t fsz = lseek(fd, 0, SEEK_END);
    if (fsz < (off_t)(40 + 24)) { werr("zpk: file too small\n", 20); return 127; }

    uint64_t tail[3]; /* nframes | payload_off | magic */
    if (pread(fd, tail, 24, fsz - 24) != 24 || memcmp((char *)tail + 16, MAGIC, 8) != 0) {
        werr("zpk: bad trailer\n", 18); return 127;
    }
    uint64_t nframes     = tail[0];
    uint64_t payload_off = tail[1];
    if (nframes == 0 || nframes > 4096) { werr("zpk: bad frame count\n", 22); return 127; }

    size_t tbl_sz = (size_t)nframes * 16;
    uint64_t *tbl = malloc(tbl_sz);
    if (!tbl || pread(fd, tbl, tbl_sz, fsz - 24 - (off_t)tbl_sz) != (ssize_t)tbl_sz) { werr("read frame table\n", 19); die(""); }

    /* map payload (contiguous frames, page-aligned offset) */
    size_t payload_len = 0, unpacked_len = 0;
    for (uint64_t i = 0; i < nframes; i++) { payload_len += tbl[2*i]; unpacked_len += tbl[2*i+1]; }
    const unsigned char *src = mmap(NULL, payload_len, PROT_READ, MAP_PRIVATE, fd, (off_t)payload_off);
    if (src == MAP_FAILED) { werr("mmap payload\n", 14); die(""); }

    int mfd = memfd_exec();
    if (mfd < 0) { werr("memfd_create\n", 15); die(""); }
    if (ftruncate(mfd, (off_t)unpacked_len) != 0) { werr("ftruncate\n", 11); die(""); }
    unsigned char *dst = mmap(NULL, unpacked_len, PROT_READ | PROT_WRITE, MAP_SHARED, mfd, 0);
    if (dst == MAP_FAILED) { werr("mmap memfd\n", 13); die(""); }

    /* parallel decompression: one thread per frame, capped */
    long maxth = ncpu();
    const char *env = getenv("ZPK_THREADS");
    if (env) { long v = atol(env); if (v >= 1) maxth = v; }
    if (maxth > (long)nframes) maxth = (long)nframes;

    pthread_t *tid = calloc(maxth > 1 ? maxth - 1 : 1, sizeof(pthread_t));
    job_t *jobs = calloc(nframes, sizeof(job_t));
    if (!tid || !jobs) { werr("alloc\n", 8); die(""); }

    size_t so = 0, doff = 0;
    uint64_t t_i = 0;
    for (uint64_t i = 0; i < nframes; i++) {
        jobs[i].src = src + so; jobs[i].dst = dst + doff;
        jobs[i].comp = tbl[2*i]; jobs[i].uncomp = tbl[2*i+1];
        so += tbl[2*i]; doff += tbl[2*i+1];
        if (t_i == (uint64_t)maxth - 1) {          /* main thread does these inline */
            worker(&jobs[i]);
            if (jobs[i].rc) { werr("zpk: frame failed\n", 19); return 127; }
        } else {
            if (pthread_create(&tid[t_i], NULL, worker, &jobs[i]) != 0) {
                worker(&jobs[i]);                   /* degrade to inline on failure */
                if (jobs[i].rc) { werr("zpk: frame failed\n", 19); return 127; }
            }
            t_i++;
        }
    }
    int rc = 0;
    for (long t = 0; t < t_i; t++) {
        job_t *j; pthread_join(tid[t], (void **)&j);
        if (j->rc) rc = 1;
    }
    if (rc) { werr("zpk: a frame failed\n", 21); return 127; }

    if (getenv("ZPK_DEBUG_SLEEP_MS")) {
        long ms = atol(getenv("ZPK_DEBUG_SLEEP_MS"));
        struct timespec ts = { ms / 1000, (ms % 1000) * 1000000L };
        nanosleep(&ts, NULL);
    }

    fexecve(mfd, argv, envp);
    werr("fexecve\n", 10); die("");
}
