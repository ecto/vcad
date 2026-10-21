"""Regenerate every figure in paper.typ from the source data.

Run with the cram bench venv (it has matplotlib):
  /Users/cam/Developer/cram/bench/.venv/bin/python make_figures.py

Data sources (read-only):
  BENCH/heldout_*.csv              per-field held-out frontiers       [B]
  BENCH/gpu_ratio.jsonl            TCG1 vs serial per field           [B]
  BENCH/gpu3_ratio.jsonl           TCG3 static per field              [B]
  ../tang-compress-m1-summary.md   held-out per-field + aggregate     [S]
  Hard-coded tables below are copied verbatim from the cited report section.
"""
import csv, glob, json, math, os, re, statistics
from collections import defaultdict

import matplotlib
matplotlib.use("svg")
import matplotlib.pyplot as plt
from matplotlib.colors import LinearSegmentedColormap, LogNorm, TwoSlopeNorm

HERE = os.path.dirname(os.path.abspath(__file__))
BENCH = "/Users/cam/Developer/cram/bench/out"
SUMMARY = os.path.join(HERE, "..", "tang-compress-m1-summary.md")

# ---- style -----------------------------------------------------------------
# master-plan palette (warm black page): text, label, hairline, the one flame
INK, MUTED, GRID = "#e8e1d2", "#a08d70", "#2a2119"
SURFACE, LINEHI = "#15110d", "#4a3a2a"
ACCENT = "#ff5f00"          # cram
C = {
    "tc-auto": ACCENT, "tc": ACCENT,
    "sperr": "#6fb7c9", "tthresh": "#9d8bc4", "sz3": "#7fae86",
    "zfp": "#5c4f40", "qtt-row": "#d9b44a", "qtt": "#d9b44a",
    "tc-wavelet": "#c9a27e", "tc-wavelet-pretrained": "#ff8a3d",
    "tc-wavelet-overfit": "#8a6a4f", "tc-tucker": "#7a5a3a",
}
LABEL = {"tc-auto": "tc-auto (ours)", "sperr": "SPERR", "tthresh": "TTHRESH",
         "sz3": "SZ3", "zfp": "ZFP", "qtt-row": "QTT (ours, M0)"}
plt.rcParams.update({
    "font.family": "sans-serif",
    "font.sans-serif": ["IBM Plex Sans", "Helvetica Neue", "DejaVu Sans"],
    "font.size": 8, "axes.edgecolor": LINEHI, "text.color": INK, "axes.labelcolor": INK,
    "xtick.color": MUTED, "ytick.color": MUTED, "axes.spines.top": False,
    "axes.spines.right": False, "axes.grid": True, "grid.color": GRID,
    "grid.linewidth": 0.6, "axes.axisbelow": True, "svg.fonttype": "none",
    "legend.frameon": False, "axes.titleweight": "normal", "axes.titlesize": 8.5,
})
DIVERGE = LinearSegmentedColormap.from_list("d", ["#2f8f5b", "#1d3a2a", "#15110d", "#5a2a12", "#ff5f00"])


def save(fig, name):
    fig.savefig(os.path.join(HERE, name + ".svg"), bbox_inches="tight", transparent=True)
    plt.close(fig)
    print("wrote", name)


def gmean(xs):
    return math.exp(statistics.fmean(math.log(x) for x in xs))


# ---- held-out data ---------------------------------------------------------
NEWAPP = ["scale-T", "scale-U", "scale-QV", "scale-PRES", "cesm-CLDTOT", "cesm-FLDS",
          "cesm-LHFLX", "cesm-TS", "exafel-ev0", "qmcpack-orb0", "qmcpack-orb100"]
SAMEAPP = ["miranda-pressure", "hurricane-V", "nyx-velocity_x"]
FIELDS = NEWAPP + SAMEAPP


def load_heldout(field):
    rows = defaultdict(list)
    for r in csv.DictReader(open(f"{BENCH}/heldout_{field}.csv")):
        if r["kind"] != "psnr" or not r["bpv"] or not r["psnr"]:
            continue
        rows[r["codec"]].append((float(r["bpv"]), float(r["psnr"])))
    return rows


def pareto(pts):
    pts = sorted(pts)
    out, best = [], -1e9
    for b, p in pts:
        if p > best:
            out.append((b, p)); best = p
    return out


def fig_rd_grid():
    codecs = ["zfp", "sz3", "qtt-row", "tthresh", "sperr", "tc-auto"]
    fig, axes = plt.subplots(3, 5, figsize=(7.2, 4.6))
    for ax, field in zip(axes.flat, FIELDS):
        rows = load_heldout(field)
        for c in codecs:
            if c not in rows:
                continue
            f = [(b, p) for b, p in pareto(rows[c]) if 30 <= p <= 115]
            if not f:
                continue
            xs, ys = zip(*f)
            ax.plot(xs, ys, color=C[c], lw=2.0 if c == "tc-auto" else 1.0,
                    zorder=5 if c == "tc-auto" else 2, marker="o" if c == "tc-auto" else None,
                    ms=2, label=LABEL[c])
        ax.set_xscale("log")
        ax.set_ylim(30, 112)
        ax.set_title(field, color=ACCENT if field in SAMEAPP else INK, loc="left")
        ax.tick_params(labelsize=6.5)
    ax = axes.flat[-1]
    ax.axis("off")
    h, l = axes.flat[0].get_legend_handles_labels()
    ax.legend(h, l, loc="center", fontsize=7.5)
    for a in axes[:, 0]:
        a.set_ylabel("PSNR (dB)")
    for a in axes[-1, :4]:
        a.set_xlabel("bits / value")
    fig.tight_layout(h_pad=0.6, w_pad=0.3)
    save(fig, "fig_rd_grid")


def parse_summary_perfield():
    """field -> point -> (ratio, best_ext) from [S] held-out per-field table."""
    out = defaultdict(dict)
    txt = open(SUMMARY).read().split("# Held-out validation")[1]
    for line in txt.splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) == 15 and cells[1] in ("new-app", "same-app") and cells[14] != "—":
            out[cells[0]][cells[2]] = (float(cells[14]), cells[13])
    return out


POINTS = ["40dB", "60dB", "80dB", "100dB", "max0.01", "max0.001", "max0.0001"]
PLABEL = ["40 dB", "60 dB", "80 dB", "100 dB", "max 1e-2", "max 1e-3", "max 1e-4"]


def fig_heatmap():
    pf = parse_summary_perfield()
    fig, ax = plt.subplots(figsize=(7.2, 4.0))
    import numpy as np
    M = np.full((len(FIELDS), len(POINTS)), np.nan)
    for i, f in enumerate(FIELDS):
        for j, p in enumerate(POINTS):
            if p in pf[f]:
                M[i, j] = pf[f][p][0]
    norm = TwoSlopeNorm(vmin=0.25, vcenter=1.0, vmax=1.6)
    im = ax.imshow(M, cmap=DIVERGE, norm=norm, aspect="auto")
    for i, f in enumerate(FIELDS):
        for j, p in enumerate(POINTS):
            if p in pf[f]:
                r, best = pf[f][p]
                ax.text(j, i - 0.12, f"{r:.2f}", ha="center", va="center", fontsize=7,
                        color=INK, weight="bold" if r > 1.01 else None)
                ax.text(j, i + 0.27, best.upper(), ha="center", va="center", fontsize=4.6,
                        color=MUTED)
            else:
                ax.text(j, i, "n/a", ha="center", va="center", fontsize=6, color=MUTED)
    ax.set_xticks(range(len(POINTS)), PLABEL)
    ax.set_yticks(range(len(FIELDS)), FIELDS)
    ax.xaxis.tick_top()
    ax.grid(False)
    ax.axhline(len(NEWAPP) - 0.5, color=MUTED, lw=1.2)
    ax.axvline(3.5, color="#0b0907", lw=2)
    for s in ax.spines.values():
        s.set_visible(False)
    cb = fig.colorbar(im, ax=ax, fraction=0.03, pad=0.02)
    cb.set_label("tc-auto bpv / best external bpv")
    cb.outline.set_visible(False)
    save(fig, "fig_heatmap")


def parse_aggregate(block="**all**"):
    txt = open(SUMMARY).read().split("# Held-out validation")[1].split(block)[1]
    out = {}
    for line in txt.splitlines()[:16]:
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) == 9 and cells[1] in ("sperr", "best-ext"):
            vals = [float(c.split()[0]) if c != "—" else None for c in cells[2:]]
            out[(cells[0], cells[1])] = vals
    return out


def fig_aggregate():
    agg = parse_aggregate()
    codecs = ["tc-tucker", "tc-wavelet", "tc-wavelet-overfit", "tc-wavelet-pretrained", "tc-auto"]
    names = {"tc-tucker": "Tucker", "tc-wavelet": "wavelet, hand", "tc-wavelet-overfit": "wavelet, overfit",
             "tc-wavelet-pretrained": "wavelet, pretrained (frozen)", "tc-auto": "tc-auto"}
    fig, axes = plt.subplots(1, 2, figsize=(7.2, 2.5), sharey=True)
    for ax, ref, title in zip(axes, ["sperr", "best-ext"], ["vs SPERR", "vs best external codec (per point)"]):
        for k, c in enumerate(codecs):
            v = agg[(c, ref)]
            xs = [j for j in range(7) if v[j] is not None]
            ax.plot(xs, [v[j] for j in xs], marker="o", ms=4 if c == "tc-auto" else 3,
                    lw=2.2 if c == "tc-auto" else 1, color=C[c], label=names[c])
            if c == "tc-auto":
                for j in xs:
                    ax.annotate(f"{v[j]:.2f}", (j, v[j]), xytext=(0, -10), textcoords="offset points",
                                ha="center", fontsize=6.5, color=ACCENT, weight="bold")
        ax.axhline(1, color=INK, lw=0.8, ls="--")
        ax.axvspan(3.5, 6.5, color=SURFACE, alpha=1, lw=0)
        ax.set_xticks(range(7), PLABEL, rotation=35, fontsize=6.5)
        ax.set_title(title, loc="left")
        ax.set_ylim(0.35, 1.7)
    axes[0].set_ylabel("geomean bpv ratio (14 held-out fields)")
    axes[1].legend(fontsize=6.5, loc="upper right")
    fig.tight_layout()
    save(fig, "fig_aggregate")


# ---- development set: learned coder [M1 §3] --------------------------------
def fig_dev_learned():
    rows = [  # field/target, SPERR, hand, pretrained, overfit   (M1 §3 table)
        ("Miranda ρ 60 dB", 0.307, 0.246, 0.231, 0.236),
        ("Miranda ρ 80 dB", 1.196, 1.064, 1.008, 0.994),
        ("Miranda ρ max 1e-4", 2.005, 1.694, 1.619, 1.605),
        ("Miranda vx 60 dB", 0.191, 0.172, 0.159, 0.165),
        ("Miranda vx 80 dB", 0.876, 0.816, 0.770, 0.769),
        ("Miranda vx max 1e-4", 1.617, 1.403, 1.339, 1.330),
        ("Hurricane U 60 dB", 0.323, 0.338, 0.328, 0.331),
        ("Hurricane U 80 dB", 1.711, 1.680, 1.667, 1.629),
        ("Hurricane U max 1e-4", 2.810, 2.592, 2.577, 2.535),
    ]
    import numpy as np
    fig, ax = plt.subplots(figsize=(7.2, 2.3))
    x = np.arange(len(rows))
    w = 0.26
    for k, (name, col) in enumerate([("hand contexts", C["tc-wavelet"]),
                                     ("pretrained MLP", C["tc-wavelet-pretrained"]),
                                     ("overfit MLP (weights counted)", C["tc-wavelet-overfit"])]):
        ys = [r[2 + k] / r[1] for r in rows]
        ax.bar(x + (k - 1) * w, ys, w, color=col, label=name)
    ax.axhline(1, color=C["sperr"], lw=1.2)
    ax.text(len(rows) - 0.5, 1.01, "SPERR", color=C["sperr"], ha="right", va="bottom", fontsize=7, weight="bold")
    ax.set_xticks(x, [r[0] for r in rows], rotation=25, ha="right", fontsize=6.5)
    ax.set_ylim(0.7, 1.08)
    ax.set_ylabel("bpv / SPERR bpv")
    ax.legend(ncol=3, fontsize=7, loc="lower left", bbox_to_anchor=(0, 1.0))
    ax.grid(axis="x", visible=False)
    fig.tight_layout()
    save(fig, "fig_dev_learned")


# ---- GPU --------------------------------------------------------------------
def load_jsonl(p):
    return [json.loads(l) for l in open(p)]


def cls(brick, two_d_map):
    return two_d_map.get(brick, brick)


def fig_gpu_perfield():
    """Per-field rate cost at 80 dB vs serial tc-wavelet, TCG1 classes and TCG3."""
    serial = {}
    tcg1 = defaultdict(dict)
    for d in load_jsonl(f"{BENCH}/gpu_ratio.jsonl"):
        if d["target"] != "p80":
            continue
        f = d["field"].split("-64x")[0].split("-1024")[0].split("-115")[0]
        if d["codec"] == "tc-wavelet":
            serial[f] = d["bpv"]
        else:
            tcg1[f][{32: "A", 256: "A", 64: "B", 512: "B", 128: "C", 1024: "C"}[d["brick"]]] = d["bpv"]
    tcg3 = defaultdict(dict)
    for d in load_jsonl(f"{BENCH}/gpu3_ratio.jsonl"):
        if d["target"] != "p80":
            continue
        f = d["field"].split("-64x")[0].split("-1024")[0].split("-115")[0]
        k = {16: "16", 64: "16", 32: "32", 128: "32", 4096: "band"}[d["brick"]]
        tcg3[f][k] = d["bpv"]
    fields = sorted(serial, key=lambda f: tcg1[f]["C"] / serial[f])
    cols = [("TCG1 32³", lambda f: tcg1[f]["A"]), ("TCG1 64³", lambda f: tcg1[f]["B"]),
            ("TCG1 128³", lambda f: tcg1[f]["C"]), ("TCG3 16³", lambda f: tcg3[f]["16"]),
            ("TCG3 32³", lambda f: tcg3[f]["32"]), ("TCG3 1 blk/band", lambda f: tcg3[f]["band"])]
    import numpy as np
    M = np.array([[g(f) / serial[f] for _, g in cols] for f in fields]).T
    print("geomean TCG1 128 / TCG3 32:", gmean(M[2]), gmean(M[4]))
    fig, ax = plt.subplots(figsize=(7.4, 2.6))
    im = ax.imshow(M, cmap=LinearSegmentedColormap.from_list("r", ["#15110d", "#4a2a14", "#b5470f", "#ff5f00"]),
                   norm=LogNorm(vmin=1.0, vmax=6.0), aspect="auto")
    for i in range(M.shape[0]):
        for j in range(M.shape[1]):
            ax.text(j, i, f"{M[i, j]:.2f}", ha="center", va="center", fontsize=5.6,
                    color=INK)
    ax.set_yticks(range(len(cols)), [c for c, _ in cols], fontsize=7)
    ax.set_xticks(range(len(fields)), fields, rotation=40, ha="right", fontsize=6.5)
    ax.grid(False)
    ax.axhline(2.5, color="#0b0907", lw=3)
    for s in ax.spines.values():
        s.set_visible(False)
    cb = fig.colorbar(im, ax=ax, fraction=0.03, pad=0.01, ticks=[1, 1.5, 2, 3, 5])
    cb.ax.set_yticklabels(["1×", "1.5×", "2×", "3×", "5×"])
    cb.ax.minorticks_off()
    cb.set_label("bpv / serial, 80 dB")
    cb.outline.set_visible(False)
    save(fig, "fig_gpu_perfield")


def fig_gpu_tradeoff():
    """Rate cost vs kernel throughput. Every point copied from [M1d]."""
    pts = [  # label, vs-serial@80dB geomean (19 fields), kernel GB/s on NYX 512³, source
        ("TCG1 32³ bricks", 1.486, (2.65, 3.35), "§3, §4"),
        ("TCG1 64³ bricks", 1.253, (1.97, 5.00), "§3, §4"),
        ("TCG1 128³ bricks", 1.152, (1.83, 1.83), "§3, §4"),
        ("TCG3 static, 1 lane", 1.107, (0.29, 0.29), "§9"),
        ("TCG3 static, 8 lanes", 1.182, (1.90, 1.90), "§9 ratio, §11 speed"),
        ("TCG3 learned 16", 1.134, (1.10, 1.10), "§10 ratio, §11 speed"),
        ("TCG3 learned 32×16", 1.118, (0.85, 0.85), "§10 ratio, §11 speed"),
    ]
    fig, ax = plt.subplots(figsize=(3.6, 2.9))
    marks = ["o", "s", "D", "^", "v", "P", "X"]
    for (lab, r, (lo, hi), _), m in zip(pts, marks):
        col = C["tc-wavelet-pretrained"] if "learned" in lab else (C["sperr"] if "TCG1" in lab else C["tthresh"])
        if hi > lo:
            ax.plot([lo, hi], [r, r], color=col, lw=3, alpha=0.35, solid_capstyle="round")
        ax.scatter([(lo * hi) ** 0.5], [r], color=col, marker=m, s=30, zorder=5, edgecolor="#0b0907", lw=0.8, label=lab)
    ax.legend(fontsize=6, loc="upper right", handletextpad=0.2, borderaxespad=0.2)
    ax.axvline(1.0, color=INK, lw=0.8, ls="--")
    ax.text(0.93, 1.51, "1 GB/s", ha="right", fontsize=6.5, color=INK)
    ax.axhline(1.05, color=ACCENT, lw=0.8, ls=":")
    ax.text(0.2, 1.056, "≤5% target", fontsize=6.5, color=ACCENT)
    ax.set_xscale("log")
    ax.set_xlim(0.18, 12)
    ax.set_ylim(1.0, 1.75)
    ax.set_xlabel("GPU kernel throughput, NYX 512³ (GB/s)")
    ax.set_ylabel("bpv vs serial coder, 80 dB")
    fig.tight_layout()
    save(fig, "fig_gpu_tradeoff")


def fig_speedup_ladder():
    """[M1d §11] kernel GB/s after each step, NYX 512³ 80 dB."""
    steps = ["baseline", "weights in\nworkgroup mem", "cached CDF\ntails", "literal-weight\nMLP", "exact 32-bit\nscale mul", "4 blocks /\nworkgroup", "quiet\nmachine"]
    big = [0.099, 0.080, 0.096, 0.492, 0.542, 0.841, 0.85]
    small = [0.222, 0.206, 0.220, 0.719, 0.821, 1.074, 1.10]
    kept = [True, False, True, True, True, True, True]
    import numpy as np
    x = np.arange(len(steps))
    fig, ax = plt.subplots(figsize=(7.2, 2.3))
    ax.bar(x - 0.2, small, 0.38, color=[C["tc-wavelet-pretrained"] if k else "#3a2f25" for k in kept], label="pretrained 16")
    ax.bar(x + 0.2, big, 0.38, color=[C["tc-wavelet-overfit"] if k else "#2a2119" for k in kept], label="pretrained 32×16")
    for xi, v in zip(x, small):
        ax.text(xi - 0.2, v + 0.02, f"{v:.2f}", ha="center", fontsize=6, color=INK)
    for xi, v in zip(x, big):
        ax.text(xi + 0.2, v + 0.02, f"{v:.2f}", ha="center", fontsize=6, color=MUTED)
    ax.axhline(1.0, color=INK, lw=0.8, ls="--")
    ax.set_xticks(x, steps, fontsize=6.5)
    ax.set_ylabel("kernel GB/s")
    ax.set_ylim(0, 1.3)
    ax.legend(fontsize=7, loc="upper left", ncol=2)
    ax.grid(axis="x", visible=False)
    ax.text(1, 0.29, "reverted", ha="center", fontsize=6, color=MUTED, style="italic")
    fig.tight_layout()
    save(fig, "fig_speedup_ladder")


def fig_throughput():
    """Decode GB/s on NYX 512³ (log). Ranges as bars. [M1d §4, §11]"""
    rows = [  # label, lo, hi, color
        ("SPERR, 1 thread", 0.095, 0.095, C["sperr"]),
        ("SPERR, 16 threads*", 0.227, 0.227, C["sperr"]),
        ("SZ3, serial", 0.59, 0.59, C["sz3"]),
        ("TCG1 64³ · CPU 16 thr", 0.37, 0.54, C["zfp"]),
        ("TCG1 64³ · GPU end-to-end", 1.40, 2.11, C["tc-wavelet"]),
        ("TCG1 64³ · GPU kernels", 1.97, 5.00, ACCENT),
        ("TCG3 learned 16 · GPU end-to-end", 0.91, 0.91, C["tc-wavelet-overfit"]),
        ("TCG3 learned 16 · GPU kernels", 1.10, 1.10, C["tc-wavelet-pretrained"]),
    ]
    fig, ax = plt.subplots(figsize=(3.6, 2.7))
    for i, (lab, lo, hi, col) in enumerate(rows):
        ax.plot([lo, hi], [i, i], color=col, lw=7, solid_capstyle="round")
        ax.text(hi * 1.12, i, f"{lo:g}" + (f"–{hi:g}" if hi > lo else ""), va="center", fontsize=6.3, color=INK)
    ax.set_yticks(range(len(rows)), [r[0] for r in rows], fontsize=6.5)
    ax.set_xscale("log")
    ax.set_xlim(0.06, 14)
    ax.axvline(1, color=INK, lw=0.8, ls="--")
    ax.set_xlabel("decode throughput (GB/s of f32)")
    ax.grid(axis="y", visible=False)
    fig.tight_layout()
    save(fig, "fig_throughput")


def fig_qtt_vs_frontier():
    """[M0 §8.3]: QTT bpv / frontier bpv. 3D fields lose, 1D algebraic signals win."""
    rows = [  # name, 60 dB ratio QTT/frontier (from §8.3 parentheses or computed from cells)
        ("Miranda ρ", 0.773 / 0.307), ("Miranda vx", 2.7), ("Hurricane U", 2.7),
        ("Hurricane TC", 14.6), ("NYX T", 6.1),
        ("1D damped sines", 0.0106 / 0.021), ("1D polynomial", 0.0022 / 0.0059),
    ]
    fig, ax = plt.subplots(figsize=(3.6, 2.2))
    ys = range(len(rows))
    for y, (n, r) in zip(ys, rows):
        ax.barh(y, math.log10(r), color=C["qtt"] if r < 1 else "#c9ced8", height=0.6)
        ax.text(math.log10(r) + (0.05 if r > 1 else -0.05), y, f"{r:.2g}×", va="center",
                ha="left" if r > 1 else "right", fontsize=6.5)
    ax.set_yticks(list(ys), [r[0] for r in rows], fontsize=6.5)
    ax.axvline(0, color=INK, lw=0.8)
    ax.set_xlabel("log₁₀ (QTT bpv / best other codec), 60 dB")
    ax.set_xlim(-0.8, 1.5)
    ax.invert_yaxis()
    ax.grid(axis="y", visible=False)
    fig.tight_layout()
    save(fig, "fig_qtt")


if __name__ == "__main__":
    fig_rd_grid(); fig_heatmap(); fig_aggregate(); fig_dev_learned()
    fig_gpu_perfield(); fig_gpu_tradeoff(); fig_speedup_ladder(); fig_throughput(); fig_qtt_vs_frontier()
