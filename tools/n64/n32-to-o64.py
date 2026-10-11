#!/usr/bin/env python3
"""Relabels LLVM-built MIPS n32 objects as o64 objects so GNU ld links them into libdragon ROMs.

    python3 -I tools/n64/n32-to-o64.py <in.a|in.o> <out.a|out.o>

Why: LLVM cannot generate libdragon's ABI (`-mabi=o64`), so Rust is compiled for n32
(same ISA: MIPS III, 64-bit GPRs, FR=1 64-bit FPRs, 32-bit pointers). GNU ld refuses to mix
ABIs ("ABI is incompatible with that of the selected emulation"), and n32 objects use RELA
relocations (elf32-nbigmips) where o64 uses REL (elf32-bigmips). The machine code itself is
ABI-neutral; only the calling convention differs, and that is bridged by the narrow FFI
(see tools/n64/README.md "ABI"). This tool, per ELF member:

  * e_flags: clears EF_MIPS_ABI2 (n32), sets EF_MIPS_ABI_O64;
  * every SHT_RELA section becomes SHT_REL (".rela.X" -> ".rel.X"), its addends moved into
    the relocated section's contents with o32/o64 REL semantics. Supported types (all that
    LLVM emits for static, non-PIC, no-small-data code): R_MIPS_32, R_MIPS_26, R_MIPS_HI16,
    R_MIPS_LO16. Each HI16 is reordered to sit right before a LO16 of the same symbol whose
    low 16 addend bits match (GNU ld pairs a HI16 with the next same-symbol LO16);
  * ".mdebug.abiN32" is renamed ".mdebug.abiO64".
Any other relocation type, or an unpairable HI16, is an error (never silently wrong).
"""
import struct
import sys

EF_MIPS_ABI2 = 0x20
EF_MIPS_ABI_MASK = 0xF000
EF_MIPS_ABI_O64 = 0x2000
SHT_RELA, SHT_REL = 4, 9
R_MIPS_32, R_MIPS_26, R_MIPS_HI16, R_MIPS_LO16 = 2, 4, 5, 6


def sext16(v):
    v &= 0xFFFF
    return v - 0x10000 if v & 0x8000 else v


def convert_elf(data, name):
    b = bytearray(data)
    if b[:4] != b"\x7fELF" or b[4] != 1 or b[5] != 2:
        raise SystemExit(f"{name}: not a big-endian ELF32 object")
    (e_flags,) = struct.unpack_from(">I", b, 36)
    e_shoff, = struct.unpack_from(">I", b, 32)
    e_shentsize, e_shnum, e_shstrndx = struct.unpack_from(">HHH", b, 46)
    if e_flags & EF_MIPS_ABI_MASK == EF_MIPS_ABI_O64 and not e_flags & EF_MIPS_ABI2:
        return bytes(b)  # already o64
    if not e_flags & EF_MIPS_ABI2:
        raise SystemExit(f"{name}: not an n32 object (e_flags {e_flags:#x})")
    struct.pack_into(">I", b, 36, (e_flags & ~EF_MIPS_ABI2 & ~EF_MIPS_ABI_MASK) | EF_MIPS_ABI_O64)

    shdrs = []
    for i in range(e_shnum):
        off = e_shoff + i * e_shentsize
        shdrs.append(list(struct.unpack_from(">IIIIIIIIII", b, off)))
    strhdr = shdrs[e_shstrndx]
    shstr = bytes(b[strhdr[4] : strhdr[4] + strhdr[5]])

    def sname(h):
        return shstr[h[0] : shstr.index(b"\0", h[0])].decode()

    newstr = bytearray(shstr)

    def add_name(s):
        o = len(newstr)
        newstr.extend(s.encode() + b"\0")
        return o

    for i, h in enumerate(shdrs):
        n = sname(h)
        if n == ".mdebug.abiN32":
            h[0] = add_name(".mdebug.abiO64")
        if h[1] != SHT_RELA:
            continue
        if h[9] != 12:
            raise SystemExit(f"{name}: {n}: unexpected RELA entsize {h[9]}")
        target = shdrs[h[7]]  # sh_info = relocated section
        tbase = target[4]
        relas = [struct.unpack_from(">IIi", b, h[4] + k * 12) for k in range(h[5] // 12)]
        out = []
        his = []
        for off, info, add in relas:
            typ = info & 0xFF
            sym = info >> 8
            p = tbase + off
            if typ == R_MIPS_32:
                (w,) = struct.unpack_from(">I", b, p)
                struct.pack_into(">I", b, p, (w + add) & 0xFFFFFFFF)
                out.append((off, info))
            elif typ == R_MIPS_26:
                (w,) = struct.unpack_from(">I", b, p)
                a = ((w & 0x3FFFFFF) << 2) + add
                struct.pack_into(">I", b, p, (w & 0xFC000000) | ((a >> 2) & 0x3FFFFFF))
                out.append((off, info))
            elif typ == R_MIPS_LO16:
                (w,) = struct.unpack_from(">I", b, p)
                if w & 0xFFFF:
                    raise SystemExit(f"{name}: {n}: LO16 with in-place addend at {off:#x}")
                struct.pack_into(">I", b, p, (w & 0xFFFF0000) | (add & 0xFFFF))
                out.append((off, info, add))
            elif typ == R_MIPS_HI16:
                (w,) = struct.unpack_from(">I", b, p)
                if w & 0xFFFF:
                    raise SystemExit(f"{name}: {n}: HI16 with in-place addend at {off:#x}")
                struct.pack_into(">I", b, p, (w & 0xFFFF0000) | (((add - sext16(add)) >> 16) & 0xFFFF))
                his.append((off, info, add))
            else:
                raise SystemExit(f"{name}: {n}: unsupported relocation type {typ} at {off:#x}")
        # Pair each HI16 with a LO16 (same symbol, same low 16 addend bits): emit it right
        # before that LO16.
        lo_index = {}
        for k, r in enumerate(out):
            if len(r) == 3 and (r[1] >> 8, r[2] & 0xFFFF) not in lo_index:
                lo_index[(r[1] >> 8, r[2] & 0xFFFF)] = k
        before = {}
        for off, info, add in his:
            k = lo_index.get((info >> 8, add & 0xFFFF))
            if k is None:
                raise SystemExit(f"{name}: {n}: HI16 at {off:#x} (sym {info >> 8}, addend {add}) has no matching LO16")
            before.setdefault(k, []).append((off, info))
        final = []
        for k, r in enumerate(out):
            final.extend(before.get(k, []))
            final.append(r[:2])
        for k, (off, info) in enumerate(final):
            struct.pack_into(">II", b, h[4] + k * 8, off, info)
        h[1] = SHT_REL
        h[5] = len(final) * 8
        h[9] = 8
        if n.startswith(".rela"):
            h[0] = add_name(".rel" + n[5:])

    # New section-name table at the end of the file.
    strhdr[4] = len(b)
    strhdr[5] = len(newstr)
    b.extend(newstr)
    for i, h in enumerate(shdrs):
        struct.pack_into(">IIIIIIIIII", b, e_shoff + i * e_shentsize, *h)
    return bytes(b)


def convert_archive(data):
    if not data.startswith(b"!<arch>\n"):
        raise SystemExit("not an ar archive")
    pos = 8
    members = []
    longnames = b""
    while pos < len(data):
        hdr = data[pos : pos + 60]
        name = hdr[:16].decode().rstrip()
        size = int(hdr[48:58].decode().strip())
        body = data[pos + 60 : pos + 60 + size]
        pos += 60 + size + (size & 1)
        if name == "/":  # symbol table: regenerated by ranlib
            continue
        if name == "//":
            longnames = body
            continue
        if name.startswith("/") and name[1:].isdigit():
            o = int(name[1:])
            name = longnames[o : longnames.index(b"/\n", o)].decode()
        else:
            name = name.rstrip("/")
        if body[:4] == b"\x7fELF":
            body = convert_elf(body, name)
        members.append((name, body))
    # Write with a GNU long-name table.
    lt = bytearray()
    offs = []
    for name, _ in members:
        offs.append(len(lt))
        lt.extend(name.encode() + b"/\n")
    out = bytearray(b"!<arch>\n")

    def member(n, body):
        out.extend(f"{n:<16}{0:<12}{0:<6}{0:<6}{644:<8}{len(body):<10}`\n".encode())
        out.extend(body)
        if len(body) & 1:
            out.extend(b"\n")

    member("//", bytes(lt))
    for (name, body), o in zip(members, offs):
        member(f"/{o}", body)
    return bytes(out)


def main(src, dst):
    data = open(src, "rb").read()
    out = convert_archive(data) if data.startswith(b"!<arch>\n") else convert_elf(data, src)
    open(dst, "wb").write(out)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
