"""Résumé d'une trace `trace-*.json` du build de profilage (#41).

    python3 tools/trace_stats.py trace-course.json [nombre de lignes]

Donne le temps de frame (moyenne, centiles, pire) et les spans qui coûtent
le plus par frame. Lit la trace ligne par ligne : une trace de plusieurs Go
passe, là où Perfetto sature.
"""

import re
import sys
from collections import defaultdict

# Une ligne par événement ; une regex plutôt que json.loads : 10× plus vite.
EVENT = re.compile(rb'"name":"((?:[^"\\]|\\.)*)","ph":"([BE])","pid":\d+,"tid":(\d+),"ts":([\d.]+)')


def main(path, top):
    stacks = defaultdict(list)  # tid -> [(nom, début)]
    total = defaultdict(float)
    count = defaultdict(int)
    worst = defaultdict(float)
    frames = []  # débuts du schedule Main
    with open(path, "rb") as f:
        for line in f:
            m = EVENT.search(line)
            if not m:
                continue
            name, ph, tid, ts = m.group(1).decode().replace('\\"', '"'), m.group(2), m.group(3), float(m.group(4))
            if ph == b"B":
                stacks[tid].append((name, ts))
                if name == "schedule: name=Main":
                    frames.append(ts)
            elif stacks[tid]:
                name, start = stacks[tid].pop()
                d = (ts - start) / 1000.0
                total[name] += d
                count[name] += 1
                worst[name] = max(worst[name], d)

    times = sorted((b - a) / 1000.0 for a, b in zip(frames, frames[1:]))
    n = len(times)
    if n == 0:
        print("aucune frame trouvée")
        return
    pct = lambda p: times[min(n - 1, int(p * n))]
    print(f"{n} frames, {sum(times) / 1000:.1f} s")
    print(f"frame : moyenne {sum(times) / n:.2f} ms, médiane {pct(0.5):.2f}, "
          f"95 % {pct(0.95):.2f}, 99 % {pct(0.99):.2f}, pire {times[-1]:.2f}")
    print(f"frames > 33 ms : {sum(t > 33.3 for t in times)}, > 100 ms : {sum(t > 100 for t in times)}")
    print()
    print(f"{'ms/frame':>9} {'pire ms':>9} {'appels':>8}  span")
    for name in sorted(total, key=total.get, reverse=True)[:top]:
        print(f"{total[name] / n:9.3f} {worst[name]:9.2f} {count[name]:8}  {name[:110]}")


if __name__ == "__main__":
    main(sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 40)
