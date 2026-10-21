#!/usr/bin/env python3
"""tang-compress launch figures. Every plotted number is read from a measured
result file (bench/out/*.csv|jsonl) or quoted verbatim from a report line;
see provenance.csv, which this script writes.

usage: make_figures.py <inputs-dir> <outputs-dir>
inputs-dir holds: heldout_*.csv, gpu3_ratio.jsonl, gpu3_learned_ratio.jsonl
"""
import csv, glob, json, math, os, sys
from collections import defaultdict
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

IN, OUT = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)
TARGET = 80.0
prov = []  # figure, series, field, quantity, value, source, row

def pareto(rows):  # copied from tang-compress bench/run_bench.py
    rows = sorted(rows, key=lambda r: r["bpv"])
    front, best = [], -1e9
    for r in rows:
        if r["psnr"] > best + 1e-9:
            front.append(r); best = r["psnr"]
    return front

def bpv_at(front, target):  # copied from bench/run_bench.py (log-linear interp)
    pts = [(r["psnr"], r["bpv"], r) for r in front if r["bpv"] > 0]
    for (p0, b0, r0), (p1, b1, r1) in zip(pts, pts[1:]):
        if p0 <= target <= p1:
            if p1 == p0:
                return b0, (r0, r1)
            t = (target - p0) / (p1 - p0)
            return math.exp(math.log(b0) + t * (math.log(b1) - math.log(b0))), (r0, r1)
    return None, None

gm = lambda xs: math.exp(sum(map(math.log, xs)) / len(xs))

# ---------- load held-out CSVs ----------
ps = defaultdict(list)
for f in sorted(glob.glob(os.path.join(IN, "heldout_*.csv"))):
    for i, r in enumerate(csv.DictReader(open(f)), start=2):
        if r["kind"] != "psnr":
            continue
        c = "qtt" if r["codec"].startswith("qtt") else r["codec"]
        r.update(bpv=float(r["bpv"]), psnr=float(r["psnr"]), n=int(r["n"]),
                 enc_s=float(r["enc_s"]), dec_s=float(r["dec_s"]),
                 _src=f"bench/out/{os.path.basename(f)}", _row=i)
        ps[(r["dataset"], c)].append(r)
FIELDS = sorted({d for d, _ in ps})
CPU = ["sperr", "sz3", "zfp", "tthresh", "qtt", "tc-auto"]

def at80(d, c):
    """bpv at 80 dB (interp on measured front) + enc/dec MB/s of the two
    bracketing measured encodes (geo-mean of their times)."""
    if (d, c) not in ps:
        return None
    b, br = bpv_at(pareto(ps[(d, c)]), TARGET)
    if b is None:
        return None
    mb = br[0]["n"] * 4 / 1e6
    dec = mb / math.sqrt(br[0]["dec_s"] * br[1]["dec_s"])
    enc = mb / math.sqrt(br[0]["enc_s"] * br[1]["enc_s"])
    return b, enc, dec, br

agg = {}
for c in CPU:
    rat, encs, decs, used = [], [], [], []
    for d in FIELDS:
        a, s = at80(d, c), at80(d, "sperr")
        if not a or not s:
            continue
        rat.append(a[0] / s[0]); encs.append(a[1]); decs.append(a[2]); used.append(d)
        for r in a[3]:
            prov.append(("A", c, d, "bracketing measured encode (psnr,bpv,enc_s,dec_s)",
                         f"{r['psnr']:.3f},{r['bpv']:.5g},{r['enc_s']:.4g},{r['dec_s']:.4g}", r["_src"], r["_row"]))
        prov.append(("A", c, d, "bpv@80dB / sperr bpv@80dB", f"{a[0]/s[0]:.4f}", "derived (log-interp)", ""))
    agg[c] = dict(y=gm(rat), dec=gm(decs), enc=gm(encs), n=len(used))
    prov.append(("A", c, "geomean", f"y=bpv ratio vs SPERR (n={len(used)})", f"{agg[c]['y']:.4f}", "derived", ""))
    prov.append(("A", c, "geomean", "decode MB/s", f"{agg[c]['dec']:.1f}", "derived", ""))
    prov.append(("A", c, "geomean", "encode MB/s", f"{agg[c]['enc']:.2f}", "derived", ""))

# ---------- GPU formats: rate on the same 14 held-out fields ----------
# 32^3 bricks for 3-D fields, 128^2 for the 2-D ones (m1d "32³ / 128²" class)
BLK = lambda r: 128 if r["field"].startswith(("cesm-", "exafel-")) else 32

def gpu_rate(fname, sel, label):
    rat = []
    for i, l in enumerate(open(os.path.join(IN, fname)), start=1):
        r = json.loads(l)
        if r["target"] != "p80" or r["field"] not in FIELDS or not sel(r):
            continue
        s, _ = bpv_at(pareto(ps[(r["field"], "sperr")]), r["psnr"])
        if s is None:
            continue
        rat.append(r["bpv"] / s)
        prov.append(("A", label, r["field"], "gpu bpv / sperr bpv at gpu's measured psnr",
                     f"{r['bpv']:.5g}/{s:.5g}={r['bpv']/s:.4f} (psnr {r['psnr']:.3f})", f"bench/out/{fname}", i))
    prov.append(("A", label, "geomean", f"y=bpv ratio vs SPERR (n={len(rat)})", f"{gm(rat):.4f}", "derived", ""))
    return gm(rat), len(rat)

g_static, n_gs = gpu_rate("gpu3_ratio.jsonl", lambda r: r["brick"] == BLK(r), "TCG3 static (GPU)")
g_learn, n_gl = gpu_rate("gpu3_learned_ratio.jsonl", lambda r: r["brick"] == BLK(r) and r["model"] == "pre16", "TCG3 learned (GPU)")
# GPU decode throughput: measured on a different field (NYX 512^3), RTX 3090,
# kernel-only, contended GPU — quoted from the tc3-cleanup report.
TC3 = "docs/compression-math-survey/sources/tang-compress-tc3-cleanup.md"
GPU = [("TCG3 static", g_static, 1732.0, n_gs, "static | 1.591 | 1.732 (median kernel GB/s)", 124),
       ("TCG3 learned", g_learn, 1101.0, n_gl, "16-hidden, APM | 0.956 | 1.101 (median kernel GB/s)", 121)]
for name, _, x, _, q, ln in GPU:
    prov.append(("A", name + " (GPU)", "nyx-temperature 512^3", "decode MB/s (GPU kernel, RTX 3090)", f"{x:.0f}", TC3, f"line {ln}: {q}"))

# ---------- styling ----------
THEMES = {
    "light": dict(bg="#FFFFFF", fg="#111111", sub="#555555", grid="#E3E3E3", accent="#E8590C",
                  base=["#4A4A4A", "#6B7A8F", "#8C8C8C", "#7D6B91", "#A0A0A0"], front="#E8590C"),
    "dark": dict(bg="#0E1116", fg="#F2F2F2", sub="#A8A8A8", grid="#2A2F37", accent="#FF8A3D",
                 base=["#D0D0D0", "#9DB0C8", "#9A9A9A", "#B8A6CC", "#7E7E7E"], front="#FF8A3D"),
}
NAMES = {"sperr": "SPERR", "sz3": "SZ3", "zfp": "ZFP", "tthresh": "TTHRESH", "qtt": "QTT", "tc-auto": "tang-compress"}

def style(ax, T):
    ax.set_facecolor(T["bg"])
    for s in ("top", "right"):
        ax.spines[s].set_visible(False)
    for s in ("left", "bottom"):
        ax.spines[s].set_color(T["sub"])
    ax.tick_params(colors=T["fg"], labelsize=15)
    ax.grid(True, which="major", color=T["grid"], lw=1)
    ax.xaxis.label.set_color(T["fg"]); ax.yaxis.label.set_color(T["fg"])

def color(c, T):
    if c == "tc-auto":
        return T["accent"]
    return T["base"][["sperr", "sz3", "zfp", "tthresh", "qtt"].index(c)]

LBL = {"tthresh": ((-16, -6), "right"), "sperr": ((0, 20), "center"), "qtt": ((14, -24), "left"), "tc-auto": ((-20, -10), "right")}

def figA(T, size, path):
    W, H = size
    fig = plt.figure(figsize=(W / 100, H / 100), facecolor=T["bg"])
    if W > H:
        ax = fig.add_axes([0.07, 0.15, 0.60, 0.68]); ax2 = fig.add_axes([0.75, 0.15, 0.22, 0.68])
    else:
        ax = fig.add_axes([0.12, 0.40, 0.84, 0.43]); ax2 = fig.add_axes([0.12, 0.12, 0.84, 0.18])
    for a in (ax, ax2):
        style(a, T); a.set_xscale("log")
    # CPU frontier (decode)
    pts = sorted(((agg[c]["dec"], agg[c]["y"], c) for c in CPU), key=lambda p: -p[0])
    front, best = [], math.inf
    for x, y, c in pts:
        if y < best:
            front.append((x, y)); best = y
    fx, fy = zip(*sorted(front))
    ax.step(fx, fy, where="post", color=T["sub"], lw=2, ls="--", zorder=1)
    ax.plot(fx, fy, "o", ms=0)
    for c in CPU:
        a = agg[c]; tc = c == "tc-auto"
        ax.scatter(a["dec"], a["y"], s=420 if tc else 220, color=color(c, T), zorder=3, edgecolor=T["bg"], lw=2)
        lab = f"{NAMES[c]}  {a['y']:.2f}×" + ("" if a["n"] == len(FIELDS) else f"  (n={a['n']})")
        off, ha = LBL.get(c, ((14, 8), "left"))
        ax.annotate(lab, (a["dec"], a["y"]), xytext=off, textcoords="offset points", ha=ha,
                    fontsize=20 if tc else 16, color=color(c, T), weight="bold" if tc else "normal")
    for name, y, x, n, _, _ in GPU:
        off = (0, 24) if "learned" in name else (0, -40)
        ax.scatter(x, y, s=260, facecolor="none", edgecolor=T["accent"], lw=3, zorder=3, marker="D")
        ax.annotate(f"{name} (GPU*)  {y:.2f}×", (x, y), xytext=off, textcoords="offset points",
                    ha="center", fontsize=15, color=T["accent"])
    ax.axhline(1.0, color=T["sub"], lw=1, alpha=0.5)
    ax.set_xlabel("decode throughput, MB/s of float32 (log)", fontsize=17)
    ax.set_ylabel("bits per value vs SPERR at 80 dB (lower = better)", fontsize=17)
    ax.set_xlim(0.5, 6000)
    ax.set_ylim(min(agg["tc-auto"]["y"], 0.6) - 0.08, max(a["y"] for a in agg.values()) * 1.12)
    # encode panel
    for c in CPU:
        a = agg[c]; tc = c == "tc-auto"
        ax2.scatter(a["enc"], a["y"], s=260 if tc else 120, color=color(c, T), zorder=3, edgecolor=T["bg"], lw=1.5)
        ax2.annotate(NAMES[c], (a["enc"], a["y"]), xytext=(9, 4), textcoords="offset points",
                     fontsize=13, color=color(c, T), weight="bold" if tc else "normal")
    ax2.set_xlabel("encode MB/s (log)", fontsize=15)
    ax2.set_ylim(ax.get_ylim()); ax2.set_xlim(min(a["enc"] for a in agg.values()) / 3, max(a["enc"] for a in agg.values()) * 6)
    ax2.axhline(1.0, color=T["sub"], lw=1, alpha=0.5)
    fig.text(0.07 if W > H else 0.12, 0.93, "tang-compress: fewer bits than SPERR at matched quality" if W > H else "tang-compress vs SPERR, SZ3, ZFP",
             fontsize=30 if W > H else 26, color=T["fg"], weight="bold")
    fig.text(0.07 if W > H else 0.12, 0.885,
             "14 held-out SDRBench fields, 80 dB PSNR, geo-mean." + (" " if W > H else "\n") + "Dashed = CPU Pareto frontier.",
             fontsize=17, color=T["sub"], va="top" if W <= H else "baseline")
    fig.text(0.07 if W > H else 0.12, 0.035 if W > H else 0.015,
             "*GPU: rate on the same 14 fields; throughput = kernel-only decode, NYX 512³, RTX 3090, shared GPU. "
             "CPU timings on a shared machine." + ("" if W > H else ""),
             fontsize=12 if W > H else 10.5, color=T["sub"], wrap=True)
    for ext in ("png", "svg"):
        fig.savefig(f"{path}.{ext}", dpi=200 if ext == "png" else 100, facecolor=T["bg"])
    plt.close(fig)

FIELDS_B = [("cesm-TS", "CESM-ATM  TS"), ("scale-T", "SCALE-LETKF  T"), ("miranda-pressure", "Miranda  pressure")]
B_CODECS = ["sz3", "zfp", "sperr", "tthresh", "qtt", "tc-auto"]
for d, _ in FIELDS_B:
    for c in B_CODECS:
        for r in pareto(ps.get((d, c), [])):
            prov.append(("B", c, d, "pareto point (bpv,psnr)", f"{r['bpv']:.5g},{r['psnr']:.3f}", r["_src"], r["_row"]))

def figB(T, size, path):
    W, H = size
    fig = plt.figure(figsize=(W / 100, H / 100), facecolor=T["bg"])
    axes = ([fig.add_axes([0.06 + i * 0.315, 0.12, 0.27, 0.70]) for i in range(3)] if W > H
            else [fig.add_axes([0.11, 0.60 - i * 0.255, 0.78, 0.19]) for i in range(3)])
    for ax, (d, title) in zip(axes, FIELDS_B):
        style(ax, T); ax.set_xscale("log")
        ends = []
        for c in B_CODECS:
            fr = [r for r in pareto(ps.get((d, c), [])) if r["bpv"] > 0 and 20 <= r["psnr"] <= 125]
            if not fr:
                continue
            tc = c == "tc-auto"
            ax.plot([r["bpv"] for r in fr], [r["psnr"] for r in fr], "-o", color=color(c, T),
                    lw=4 if tc else 2, ms=6 if tc else 4, zorder=3 if tc else 2)
            ends.append((fr[-1]["bpv"], fr[-1]["psnr"], c))
        ends.sort(key=lambda e: -min(e[1], 118)); ly = []
        for x, y, c in ends:
            y = min(y, 118)
            ly.append(min(y, ly[-1] - 5.5) if ly else y)
        xr = max(e[0] for e in ends if e[1] < 125) * 1.6
        for (x, y, c), yl in zip(ends, ly):
            ax.annotate(NAMES[c], (min(x, xr), min(y, 118)), xytext=(xr, yl), textcoords="data",
                        arrowprops=dict(arrowstyle="-", color=color(c, T), lw=0.8, alpha=0.6), va="center",
                        fontsize=14 if c == "tc-auto" else 12, color=color(c, T),
                        weight="bold" if c == "tc-auto" else "normal")
        ax.set_title(title, fontsize=18, color=T["fg"], loc="left")
        ax.set_ylim(30, 122)
        ax.set_xlabel("bits per value (log)", fontsize=14)
        xs = [r["bpv"] for c in B_CODECS for r in pareto(ps.get((d, c), [])) if r["bpv"] > 0 and 30 <= r["psnr"] <= 122]
        ax.set_xlim(min(xs) * 0.6, max(xs) * 12)
    axes[0].set_ylabel("PSNR (dB)", fontsize=15) if W > H else [a.set_ylabel("PSNR (dB)", fontsize=13) for a in axes]
    x0 = 0.06 if W > H else 0.11
    fig.text(x0, 0.92, "Rate–distortion on held-out fields", fontsize=30 if W > H else 26, color=T["fg"], weight="bold")
    fig.text(x0, 0.87 if W > H else 0.885, "Measured Pareto fronts; up and to the left is better. tang-compress = frozen tc-auto.",
             fontsize=17 if W > H else 14, color=T["sub"])
    for ext in ("png", "svg"):
        fig.savefig(f"{path}.{ext}", dpi=200 if ext == "png" else 100, facecolor=T["bg"])
    plt.close(fig)

for tn, T in THEMES.items():
    for sn, size in (("16x9", (1920, 1080)), ("square", (1080, 1080))):
        figA(T, size, f"{OUT}/figA-rate-vs-speed-{sn}-{tn}")
        figB(T, size, f"{OUT}/figB-rate-distortion-{sn}-{tn}")

with open(f"{OUT}/provenance.csv", "w", newline="") as fh:
    w = csv.writer(fh); w.writerow(["figure", "series", "field", "quantity", "value", "source_file", "source_row"])
    w.writerows(prov)
with open(f"{OUT}/plotted_numbers.json", "w") as fh:
    json.dump(dict(cpu=agg, gpu=[dict(name=g[0], y=g[1], dec_mbs=g[2], n=g[3]) for g in GPU]), fh, indent=1)
print(json.dumps(dict(cpu=agg, gpu=[g[:4] for g in GPU]), indent=1))
