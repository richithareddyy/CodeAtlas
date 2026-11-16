"""Summarises results/*-comparison.json into the tables in README.md.

    python3 benchmarks/external/analyze.py [primary|held_out]
"""

import json
import sys
from collections import Counter
from pathlib import Path

here = Path(__file__).parent
probe_set = sys.argv[1] if len(sys.argv) > 1 else "primary"
repos = [r["name"] for r in json.loads((here / "repos.json").read_text())["repositories"]]
results = {}
for name in repos:
    path = here / "results" / f"{name}-{probe_set}-comparison.json"
    if path.exists():
        results[name] = json.loads(path.read_text())


def fmt(x):
    return "n/a" if x is None else f"{x:.3f}"


print(f"# {probe_set}\n")
print("| Repository | Tests | Invisible | Probes ok / total | Executed | Resolution rate | Ambiguous calls |")
print("|---|---:|---:|---:|---:|---:|---:|")
for name, c in results.items():
    p = c["probes"]
    print(
        f"| {name} | {c['tests']} | {c['invisible_tests']} | {p['ok']} / {p['total']} "
        f"| {p['ok'] - p['not_executed']} | {fmt(c['resolution']['resolution_rate'])} "
        f"| {c['resolution']['ambiguous']} |"
    )

print("\n## Strategies per repository\n")
print("| Repository | Strategy | Recall | Safe probes | Precision | Mean selected | Share | Test time saved | Missed invisible | Missed visible |")
print("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|")
for name, c in results.items():
    for s in c["strategies"]:
        print(
            f"| {name} | {s['name']} | {fmt(s['recall'])} | {s['safe_probes']}/{s['probes_with_failures']} "
            f"| {fmt(s['precision'])} | {s['mean_selected']:.1f} | {fmt(s['mean_selected_share'])} "
            f"| {fmt(s['mean_time_saved'])} | {s['missed_invisible']} | {s['missed_visible']} |"
        )

print("\n## Pooled over repositories (probes with failures)\n")
print("| Strategy | Recall | Safe probes | Missed invisible | Missed visible |")
print("|---|---:|---:|---:|---:|")
names = [s["name"] for s in next(iter(results.values()))["strategies"]] if results else []
for strategy in names:
    rows = [s for c in results.values() for s in c["strategies"] if s["name"] == strategy]
    if len(rows) != len(results):
        continue
    failing = sum(r["failing"] for r in rows)
    tp = sum(r["true_positives"] for r in rows)
    safe = sum(r["safe_probes"] for r in rows)
    with_failures = sum(r["probes_with_failures"] for r in rows)
    print(
        f"| {strategy} | {fmt(tp / failing if failing else None)} | {safe}/{with_failures} "
        f"| {sum(r['missed_invisible'] for r in rows)} | {sum(r['missed_visible'] for r in rows)} |"
    )

print("\n## Conservative rule\n")
print("| Repository | Allowed selective | Allowed and missed a failing test | Reasons fired (probes) |")
print("|---|---:|---:|---|")
for name, c in results.items():
    allowed = [p for p in c["per_probe"] if p["selective"]]
    missed = [
        p for p in allowed
        if p["outcomes"]["codeatlas"]["true_positives"] < p["failing"]
    ]
    reasons = ", ".join(f"{k} {v}" for k, v in sorted(c["reasons"].items()))
    print(f"| {name} | {len(allowed)}/{len(c['per_probe'])} | {len(missed)} | {reasons} |")

print("\n## CodeAtlas misses by kind\n")
print("| Repository | Failing | Invisible missed | Visible missed | ...found with ambiguous calls | ...missed even then |")
print("|---|---:|---:|---:|---:|---:|")
for name, c in results.items():
    failing = inv = vis = via_amb = still = 0
    for p in c["per_probe"]:
        failing += p["failing"]
        o = p["outcomes"]["codeatlas"]
        inv += o["missed_invisible"]
        vis += len(o["missed_visible"])
        still_missed = set(p["outcomes"]["codeatlas-ambiguous"]["missed_visible"])
        for t in o["missed_visible"]:
            if t in still_missed:
                still += 1
            else:
                via_amb += 1
    print(f"| {name} | {failing} | {inv} | {vis} | {via_amb} | {still} |")

print("\n## Visible tests missed even with ambiguous calls\n")
for name, c in results.items():
    for p in c["per_probe"]:
        missed = p["outcomes"]["codeatlas-ambiguous"]["missed_visible"]
        if missed:
            reasons = ",".join(sorted(set(p["reasons"]))) or "none"
            print(f"- {name}: `{p['symbol']}` ({len(missed)}; rule: {reasons}): " + ", ".join(f"`{t}`" for t in missed[:3]) + (" ..." if len(missed) > 3 else ""))

print("\n## Invisible failing tests by crate\n")
for name, c in results.items():
    truth = json.loads((here / "raw" / f"{name}-{probe_set}-truth.json").read_text())
    crates = Counter()
    for p in truth["probes"]:
        for t in p["failing"]:
            if t.startswith("unmapped:"):
                crates[t[len("unmapped:"):].split("::")[0]] += 1
    if crates:
        print(f"- {name}: " + ", ".join(f"{k} {v}" for k, v in crates.most_common()))
