#!/usr/bin/env python3
"""zpack: append zstd-compressed payloads to a self-extracting stub.

Two container flavors:
  ELF (default)  magic "ZPK2zstd" — frames tile the flat file 1:1;
                 table stride 16 B {comp u64, uncomp u64}.
  PE (--pe)      magic "ZPK3pe64" — frames tile the flat payload file 1:1
                 (headers, sections, alignment padding, trailing overlay);
                 table stride 24 B {comp u64, uncomp u64, dest u64} where
                 dest is the frame's offset in the reconstructed file.
                 Also copies the payload's Subsystem into the stub so GUI
                 payloads don't get a console window.

Usage: zpack.py <stub> <input> <output> [--frames N] [--level N] [--pe] [--page P]
"""
import argparse
import os
import struct
import subprocess


def die(msg):
    raise SystemExit(f"zpack: {msg}")


def parse_pe64(data):
    if data[:2] != b"MZ":
        die("--pe payload is not an MZ/PE image")
    try:
        return _parse_pe64(data)
    except struct.error:
        die("--pe payload is truncated (header/section table past EOF)")


def _parse_pe64(data):
    e = struct.unpack_from("<I", data, 0x3C)[0]
    if data[e:e + 4] != b"PE\0\0":
        die("--pe payload has no PE signature")
    coff = e + 4
    nsec = struct.unpack_from("<H", data, coff + 2)[0]
    optsz = struct.unpack_from("<H", data, coff + 16)[0]
    opt = coff + 20
    (magic,) = struct.unpack_from("<H", data, opt)
    if magic != 0x20B:
        die("--pe requires PE32+ (x64)")
    size_of_headers = struct.unpack_from("<I", data, opt + 60)[0]
    subsystem = struct.unpack_from("<H", data, opt + 68)[0]
    secs = []
    so = opt + optsz
    if so + 40 * nsec > len(data):
        die("--pe payload section table runs past end of file")
    for i in range(nsec):
        off = so + 40 * i
        vsize, va, rawsz, ptrraw = struct.unpack_from("<IIII", data, off + 8)
        if rawsz and ptrraw:
            if ptrraw + rawsz > len(data):
                die(f"--pe section {i} raw data ({ptrraw:#x}+{rawsz:#x}) runs past end of file")
            secs.append((va, vsize, rawsz, ptrraw))
    return size_of_headers, subsystem, secs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("stub")
    ap.add_argument("input")
    ap.add_argument("output")
    ap.add_argument("--frames", type=int, default=4)
    ap.add_argument("--level", type=int, default=19)
    ap.add_argument("--page", type=int, default=4096,
                    help="stub padding (65536 for 64K-page ARM, ELF flow only)")
    ap.add_argument("--pe-chunk", type=float, default=4.0,
                    help="PE mode: max MiB per frame (large sections are split for balance)")
    ap.add_argument("--pe", action="store_true",
                    help="PE32+ payload: image-layout frames + subsystem passthrough")
    args = ap.parse_args()
    if not 1 <= args.level <= 22:
        die(f"--level must be 1..22 (got {args.level})")
    if args.pe_chunk <= 0:
        die(f"--pe-chunk must be > 0 MiB (got {args.pe_chunk})")
    if args.page <= 0 or args.page % 4096:
        die(f"--page must be a positive multiple of 4096 (got {args.page})")

    with open(args.input, "rb") as f:
        plain = f.read()
    if not plain:
        die(f"input {args.input!r} is empty")

    frames = []  # (plain_bytes, dest_off)
    if args.pe:
        size_of_headers, subsystem, secs = parse_pe64(plain)
        # segments that carry raw file data
        segs = [(0, size_of_headers)] if size_of_headers else []
        for _va, _vsize, rawsz, ptrraw in secs:
            segs.append((ptrraw, rawsz))
        segs.sort()
        # tile the file 1:1: cover alignment padding between segments and
        # any trailing overlay (e.g. signature tables) so extract-to-temp
        # reconstructs the original bytes exactly
        tiled = []
        cur = 0
        for off, ln in segs:
            if off < cur:
                die("overlapping raw sections are not supported")
            if off > cur:
                tiled.append((cur, off - cur))
            tiled.append((off, ln))
            cur = max(cur, off + ln)
        if cur < len(plain):
            tiled.append((cur, len(plain) - cur))
        # balance: split segments so no worker gets stuck on a big .text
        piece = int(args.pe_chunk * 1024 * 1024)
        for off, ln in tiled:
            n = (ln + piece - 1) // piece if ln > piece else 1
            sub = (ln + n - 1) // n
            for o in range(0, ln, sub):
                frames.append((plain[off + o:off + o + min(sub, ln - o)], off + o))
    else:
        total = len(plain)
        n = max(1, args.frames)
        chunk = (total + n - 1) // n
        for i in range(0, total, chunk):
            c = plain[i:i + chunk]
            frames.append((c, i))

    comp_frames = []  # (comp, uncomp, dest)
    for c, dest in frames:
        comp = subprocess.run(["zstd", f"-{args.level}", "-T1", "-q"],
                              input=c, capture_output=True, check=True).stdout
        comp_frames.append((comp, len(c), dest))

    with open(args.stub, "rb") as f:
        stub = bytearray(f.read())
    if args.pe:
        # passthrough the payload's subsystem (GUI payload => no console window)
        e = struct.unpack_from("<I", stub, 0x3C)[0]
        if stub[e:e + 4] == b"PE\0\0" and struct.unpack_from("<H", stub, e + 24)[0] == 0x20B:
            struct.pack_into("<H", stub, e + 24 + 68, subsystem)
    pad = (-len(stub)) % args.page
    stub += b"\0" * pad
    payload_off = len(stub)

    magic = b"ZPK3pe64" if args.pe else b"ZPK2zstd"
    with open(args.output, "wb") as f:
        f.write(stub)
        for comp, _unc, _dest in comp_frames:
            f.write(comp)
        for comp, unc, dest in comp_frames:
            if args.pe:
                f.write(struct.pack("<QQQ", len(comp), unc, dest))
            else:
                f.write(struct.pack("<QQ", len(comp), unc))
        f.write(struct.pack("<Q", len(comp_frames)))
        f.write(struct.pack("<Q", payload_off))
        f.write(magic)
    os.chmod(args.output, 0o755)
    sz = os.path.getsize(args.output)
    mode = (f"PE32+ chunk<={args.pe_chunk:g}MiB" if args.pe else "ELF")
    print(f"zpack: {len(plain):,} -> {sz:,} B ({sz / len(plain) * 100:.1f}%) "
          f"[zstd -{args.level}, {len(comp_frames)} frames, {mode}, payload "
          f"{sum(len(c) for c, _, _ in comp_frames) / 1e6:.2f}MB, stub {payload_off / 1e3:.0f}kB]")


if __name__ == "__main__":
    main()
