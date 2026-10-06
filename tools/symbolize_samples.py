import bisect
import collections
import subprocess
import sys


def load_maps(path):
    maps = []
    for line in open(path):
        parts = line.split()
        if len(parts) < 6 or "x" not in parts[1]:
            continue
        lo, hi = (int(x, 16) for x in parts[0].split("-"))
        maps.append((lo, hi, int(parts[2], 16), parts[5]))
    maps.sort()
    return maps


def load_segments(path):
    out = subprocess.run(["llvm-readelf", "-lW", path], capture_output=True, text=True).stdout
    segs = []
    for line in out.splitlines():
        cols = line.split()
        if cols and cols[0] == "LOAD":
            segs.append((int(cols[1], 16), int(cols[2], 16), int(cols[4], 16)))
    return segs


def main():
    args = sys.argv[1:]
    top = 40
    focus = None
    if "--top" in args:
        i = args.index("--top")
        top = int(args[i + 1])
        del args[i : i + 2]
    if "--focus" in args:
        i = args.index("--focus")
        focus = args[i + 1]
        del args[i : i + 2]
    debug_dir = None
    if "--debug-dir" in args:
        i = args.index("--debug-dir")
        debug_dir = args[i + 1]
        del args[i : i + 2]
    prefix, roots = args[0], [r.split("=", 1) for r in args[1:]]

    maps = load_maps(prefix + ".maps")
    starts = [m[0] for m in maps]
    samples = [[int(a, 16) for a in line.split()] for line in open(prefix + ".samples") if line.strip()]

    def local(path):
        for remote, here in roots:
            if path.startswith(remote):
                return here + path[len(remote) :]
        return path

    per_obj = collections.defaultdict(set)
    where = {}
    seg_cache = {}
    for sample in samples:
        for addr in sample:
            if addr in where:
                continue
            i = bisect.bisect_right(starts, addr) - 1
            if i < 0 or addr >= maps[i][1]:
                where[addr] = None
                continue
            lo, _, off, path = maps[i]
            obj = local(path)
            if obj not in seg_cache:
                seg_cache[obj] = load_segments(obj)
            file_off = addr - lo + off
            vaddr = file_off
            for seg_off, seg_vaddr, seg_size in seg_cache[obj]:
                if seg_off <= file_off < seg_off + seg_size:
                    vaddr = file_off - seg_off + seg_vaddr
                    break
            where[addr] = (obj, vaddr)
            per_obj[obj].add(vaddr)

    names = {}
    for obj, addrs in per_obj.items():
        addrs = sorted(addrs)
        query = "\n".join(hex(a) for a in addrs)
        sym_obj = obj
        if debug_dir:
            import os
            candidate = debug_dir + "/usr/lib/aarch64-linux-gnu/" + obj.rsplit("/", 1)[-1] + ".debug"
            if os.path.exists(candidate):
                sym_obj = candidate
        out = subprocess.run(["llvm-symbolizer", "--obj=" + sym_obj, "--functions=linkage", "--demangle", "--no-inlines"], input=query, capture_output=True, text=True).stdout
        blocks = out.strip().split("\n\n")
        short = obj.rsplit("/", 1)[-1]
        for a, block in zip(addrs, blocks):
            fn = block.split("\n")[0]
            if fn in ("??", ""):
                fn = f"{short}+{a:#x}"
            names[(obj, a)] = fn

    def name(addr):
        w = where.get(addr)
        return names.get(w, "?") if w else "?"

    named = [[name(a) for a in s] for s in samples]
    total = len(named)
    self_c = collections.Counter(s[0] for s in named if s)
    incl = collections.Counter()
    for s in named:
        for fn in set(s):
            incl[fn] += 1
    print(f"{total} samples (1 ms each)\n\n== self ==")
    for fn, n in self_c.most_common(top):
        print(f"{n:6} {100*n/total:5.1f}%  {fn[:150]}")
    print("\n== total ==")
    for fn, n in incl.most_common(top):
        print(f"{n:6} {100*n/total:5.1f}%  {fn[:150]}")
    if focus:
        callers, callees = collections.Counter(), collections.Counter()
        hits = 0
        for s in named:
            idx = [i for i, fn in enumerate(s) if focus in fn]
            if not idx:
                continue
            hits += 1
            i = idx[-1]
            if i + 1 < len(s):
                callers[s[i + 1]] += 1
            j = idx[0]
            if j > 0:
                callees[s[j - 1]] += 1
        print(f"\n== {focus}: {hits} samples; callers ==")
        for fn, n in callers.most_common(top):
            print(f"{n:6}  {fn[:150]}")
        print("== callees ==")
        for fn, n in callees.most_common(top):
            print(f"{n:6}  {fn[:150]}")


main()
