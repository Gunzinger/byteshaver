#!/usr/bin/env python3
"""zpack v2: split into nframes independent zstd frames + MT stub.

Usage: zpack2.py <stub> <input> <output> [--frames N] [--level N] [--page P]
"""
import argparse
import os
import struct
import subprocess
import sys

MAGIC = b"ZPK2zstd"

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("stub")
    ap.add_argument("input")
    ap.add_argument("output")
    ap.add_argument("--frames", type=int, default=4)
    ap.add_argument("--level", type=int, default=19)
    ap.add_argument("--page", type=int, default=4096, help="stub padding (65536 for 64K-page ARM)")
    args = ap.parse_args()

    with open(args.input, "rb") as f:
        plain = f.read()

    total = len(plain)
    n = max(1, args.frames)
    chunk = (total + n - 1) // n
    chunks = []
    for i in range(0, total, chunk):
        c = plain[i:i + chunk]
        comp = subprocess.run(["zstd", f"-{args.level}", "-T1", "-q"],
                              input=c, capture_output=True, check=True).stdout
        chunks.append((comp, len(c)))

    with open(args.stub, "rb") as f:
        stub = f.read()
    pad = (-len(stub)) % args.page
    stub += b"\0" * pad
    payload_off = len(stub)

    with open(args.output, "wb") as f:
        f.write(stub)
        for comp, _ in chunks:
            f.write(comp)
        for comp, unc in chunks:
            f.write(struct.pack("<QQ", len(comp), unc))
        f.write(struct.pack("<Q", len(chunks)))
        f.write(struct.pack("<Q", payload_off))
        f.write(MAGIC)
    os.chmod(args.output, 0o755)
    sz = os.path.getsize(args.output)
    print(f"zpack2: {total:,} -> {sz:,} B ({sz/total*100:.1f}%) "
          f"[{len(chunks)} frames, payload {sum(len(c) for c,_ in chunks)/1e6:.2f}MB, stub {payload_off/1e3:.0f}kB]")

if __name__ == "__main__":
    main()
