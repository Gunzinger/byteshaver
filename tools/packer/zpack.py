#!/usr/bin/env python3
"""zpack: append zstd-compressed payloads to a self-extracting stub.

Two container flavors:
  ELF (default)  magic "ZPK2zstd" — frames tile the flat file 1:1;
                 table stride 16 B {comp u64, uncomp u64}.
  PE (--pe)      magic "ZPK3pes"  — frame 0 = PE headers, one frame per
                 section, destinations are RVAs of the target image;
                 table stride 24 B {comp u64, uncomp u64, dest_rva u64}.
                 Also copies the payload's Subsystem into the stub so GUI
                 payloads don't get a console window.

Usage: zpack.py <stub> <input> <output> [--frames N] [--level N] [--pe] [--page P]
"""
import argparse
import os
import struct
import subprocess


def parse_pe64(data):
    if data[:2] != b"MZ":
        raise SystemExit("zpack: --pe payload is not an MZ/PE image")
    e = struct.unpack_from("<I", data, 0x3C)[0]
    if data[e:e + 4] != b"PE\0\0":
        raise SystemExit("zpack: --pe payload has no PE signature")
    coff = e + 4
    nsec = struct.unpack_from("<H", data, coff + 2)[0]
    optsz = struct.unpack_from("<H", data, coff + 16)[0]
    opt = coff + 20
    (magic,) = struct.unpack_from("<H", data, opt)
    if magic != 0x20B:
        raise SystemExit("zpack: --pe requires PE32+ (x64)")
    size_of_headers = struct.unpack_from("<I", data, opt + 60)[0]
    subsystem = struct.unpack_from("<H", data, opt + 68)[0]
    secs = []
    so = opt + optsz
    for i in range(nsec):
        off = so + 40 * i
        vsize, va, rawsz, ptrraw = struct.unpack_from("<IIII", data, off + 8)
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

    with open(args.input, "rb") as f:
        plain = f.read()

    frames = []  # (plain_bytes, dest_rva, copy_len)
    if args.pe:
        size_of_headers, subsystem, secs = parse_pe64(plain)
        frames.append((plain[:size_of_headers], 0, size_of_headers))
        for va, vsize, rawsz, ptrraw in sorted(secs, key=lambda s: s[3]):
            if not (rawsz and ptrraw):
                continue
            raw = plain[ptrraw:ptrraw + rawsz]
            # balance: split big sections so no worker gets stuck on .text;
            # dest rva = section va + sub-offset (loader maps raw contiguously)
            piece = int(args.pe_chunk * 1024 * 1024)
            if len(raw) <= piece:
                frames.append((raw, va, rawsz))
            else:
                n = (len(raw) + piece - 1) // piece
                sub = (len(raw) + n - 1) // n
                for o in range(0, len(raw), sub):
                    frames.append((raw[o:o + sub], va + o, rawsz))
    else:
        total = len(plain)
        n = max(1, args.frames)
        chunk = (total + n - 1) // n
        for i in range(0, total, chunk):
            c = plain[i:i + chunk]
            frames.append((c, 0, len(c)))

    comp_frames = []  # (comp, uncomp, dest_rva, copy_len)
    for c, rva, copy_len in frames:
        comp = subprocess.run(["zstd", f"-{args.level}", "-T1", "-q"],
                              input=c, capture_output=True, check=True).stdout
        comp_frames.append((comp, len(c), rva, copy_len))

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
        for comp, unc, rva, cl in comp_frames:
            f.write(comp)
        for comp, unc, rva, cl in comp_frames:
            if args.pe:
                f.write(struct.pack("<QQQ", len(comp), unc, rva))
            else:
                f.write(struct.pack("<QQ", len(comp), unc))
        f.write(struct.pack("<Q", len(comp_frames)))
        f.write(struct.pack("<Q", payload_off))
        f.write(magic)
    os.chmod(args.output, 0o755)
    sz = os.path.getsize(args.output)
    print(f"zpack: {len(plain):,} -> {sz:,} B ({sz / len(plain) * 100:.1f}%) "
          f"[{len(comp_frames)} frames, payload "
          f"{sum(len(c) for c, _, _, _ in comp_frames) / 1e6:.2f}MB, stub {payload_off / 1e3:.0f}kB]")


if __name__ == "__main__":
    main()
