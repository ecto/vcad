"""Visual demos for paper-draft.typ: equal-rate reconstructions and error maps.

Unlike make_figures.py, these are NEW runs (this paper), not copied numbers.
Every measured point is written to demo_results.json, and the figures read it.

  /Users/cam/Developer/tang-compress/bench/.venv/bin/python make_demos.py

Codecs are the same binaries/libraries as the benchmark harness (tc example at
the tang branch tip, SPERR b801258, SZ3 via pysz, ZFP via zfpy). Each codec is
tuned on its own knob to land at or just under a target bpv; all errors are
measured here on the decoded output; rate = full stream size.
Scratch files go to $TMPDIR (never into the tang-compress tree).
"""
import json, math, os, subprocess, sys, tempfile
import numpy as np
import zfpy
from pysz import sz, szConfig, szErrorBoundMode

HERE = os.path.dirname(os.path.abspath(__file__))
TANG = "/Users/cam/Developer/tang-compress"
TC = f"{TANG}/target/release/examples/tc"
SP = f"{TANG}/bench/third_party/SPERR/build/bin"
DATA = f"{TANG}/bench/data"
TMP = tempfile.mkdtemp(prefix="tcdemo-")


def metrics(x, y):
    e = (y.astype(np.float64) - x)
    rng = float(x.max() - x.min())
    rmse = float(np.sqrt(np.mean(e ** 2)))
    return {"psnr": 20 * math.log10(rng / rmse) if rmse > 0 else float("inf"), "max_rel": float(np.abs(e).max() / rng)}


def tc(x, args):
    src, bs, out = f"{TMP}/in.f32", f"{TMP}/tc.bs", f"{TMP}/out.f32"
    x.astype("<f4").tofile(src)
    subprocess.run([TC, "encode", src, bs, "--dims", "x".join(map(str, x.shape)), "--codec", "auto", "--model", "overfit", *args],
                   check=True, capture_output=True)
    subprocess.run([TC, "decode", bs, out], check=True, capture_output=True)
    return 8 * os.path.getsize(bs) / x.size, np.fromfile(out, "<f4").reshape(x.shape)


def sperr(x, knob):
    src, bs, out = f"{TMP}/sp.f32", f"{TMP}/sp.bs", f"{TMP}/spo.f32"
    x.astype("<f4").tofile(src)
    exe, extra = (f"{SP}/sperr2d", []) if x.ndim == 2 else (f"{SP}/sperr3d", ["--omp", "1"])
    subprocess.run([exe, "-c", "--ftype", "32", "--dims", *map(str, x.shape[::-1]), *knob, "--bitstream", bs, *extra, src],
                   check=True, capture_output=True)
    subprocess.run([exe, "-d", "--decomp_f", out, *extra, bs], check=True, capture_output=True)
    return 8 * os.path.getsize(bs) / x.size, np.fromfile(out, "<f4").reshape(x.shape)


def sz3(x, absb):
    c = szConfig()
    c.errorBoundMode = szErrorBoundMode.ABS
    c.absErrorBound = absb
    cmp, _ = sz.compress(x, c)
    y, _ = sz.decompress(cmp, np.float32, x.shape)
    return 8 * cmp.nbytes / x.size, y


def zfp(x, tol):
    b = zfpy.compress_numpy(x, tolerance=tol)
    return 8 * len(b) / x.size, zfpy.decompress_numpy(b)


def at_rate(fn, lo, hi, target, log=True, increasing=True, it=22):
    """Bisect a monotone knob so bpv lands at or just under target."""
    best = None
    for _ in range(it):
        mid = math.sqrt(lo * hi) if log else (lo + hi) / 2
        bpv, y = fn(mid)
        if bpv <= target:
            if best is None or bpv > best[0]:
                best = (bpv, y)
            lo, hi = (mid, hi) if increasing else (lo, mid)
        else:
            lo, hi = (lo, mid) if increasing else (mid, hi)
    return best


def equal_rate(x, target):
    rng = float(x.max() - x.min())
    runs = {
        "tc-auto": at_rate(lambda p: tc(x, ["--psnr", f"{p:.4f}"]), 15, 120, target, log=False, increasing=True),
        "SPERR": sperr(x, ["--bpp", repr(target)]),
        "SZ3": at_rate(lambda b: sz3(x, b), rng * 1e-7, rng, target, increasing=False),
        # ZFP cannot reach very low rates; fall back to its lowest-rate point (flagged in the figure)
        "ZFP": at_rate(lambda t: zfp(x, t), rng * 1e-7, rng * 10, target, increasing=False) or zfp(x, rng * 10),
    }
    return runs


def bounded(x, rel):
    b = rel * float(x.max() - x.min())
    return {"tc-auto": tc(x, ["--max-rel", repr(rel)]), "SPERR": sperr(x, ["--pwe", repr(b)]),
            "SZ3": sz3(x, b), "ZFP": at_rate(lambda t: zfp(x, t), b * 1e-3, b * 4, 1e9, increasing=False)}


def zfp_bounded(x, b):
    """ZFP's best honest mode: largest tolerance whose actual max error <= b (as in the harness)."""
    lo, hi, best = b * 1e-3, b * 4, None
    for _ in range(30):
        t = math.sqrt(lo * hi)
        bpv, y = zfp(x, t)
        if np.abs(y.astype(np.float64) - x).max() <= b:
            best, lo = (bpv, y), t
        else:
            hi = t
    return best


def main():
    res, slices = {}, {}
    cesm = np.fromfile(f"{DATA}/heldout/cesm-TS.f32", "<f4").reshape(1024, 1024)
    mir = np.ascontiguousarray(np.fromfile(f"{DATA}/miranda-density.d64", "<f8").reshape(256, 384, 384)[96:160, 128:256, 128:256].astype(np.float32))
    for name, x, target in [("cesm-TS", cesm, 0.03), ("miranda-density", mir, 0.05)]:
        print(name, flush=True)
        runs = equal_rate(x, target)
        res[name] = {"target_bpv": target, "shape": list(x.shape), "runs": {}}
        sl = (lambda a: a) if x.ndim == 2 else (lambda a: a[32])
        slices[f"{name}/orig"] = sl(x)
        for k, (bpv, y) in runs.items():
            res[name]["runs"][k] = {"bpv": bpv, **metrics(x, y)}
            slices[f"{name}/{k}"] = sl(y)
            print(f"  {k:8s} bpv={bpv:.4f} {res[name]['runs'][k]}", flush=True)
    # pointwise bound demo on cesm-TS, rel 1e-3
    b = 1e-3 * float(cesm.max() - cesm.min())
    bd = {"tc-auto": tc(cesm, ["--max-rel", "0.001"]), "SPERR": sperr(cesm, ["--pwe", repr(b)]),
          "SZ3": sz3(cesm, b), "ZFP": zfp_bounded(cesm, b)}
    res["cesm-TS-maxabs-1e-3"] = {"bound_rel": 1e-3, "runs": {}}
    for k, (bpv, y) in bd.items():
        res["cesm-TS-maxabs-1e-3"]["runs"][k] = {"bpv": bpv, **metrics(cesm, y)}
        slices[f"bound/{k}"] = ((y.astype(np.float64) - cesm) / b).astype(np.float32)
        print(f"  bound {k:8s} bpv={bpv:.4f} {res['cesm-TS-maxabs-1e-3']['runs'][k]}", flush=True)
    json.dump(res, open(os.path.join(HERE, "demo_results.json"), "w"), indent=1)
    np.savez_compressed(os.path.join(TMP, "slices.npz"), **slices)
    print("slices", os.path.join(TMP, "slices.npz"))
    plot(res, slices)


# ---------------------------------------------------------------- plotting
def plot(res, slices):
    import matplotlib
    matplotlib.use("svg")
    import matplotlib.pyplot as plt
    from matplotlib.colors import LogNorm
    sys.path.insert(0, HERE)
    import make_figures as mf  # shared palette + rcParams
    INK, MUTED, ACCENT = mf.INK, mf.MUTED, mf.ACCENT
    order = ["tc-auto", "SPERR", "SZ3", "ZFP"]
    col = {"tc-auto": ACCENT, "SPERR": mf.C["sperr"], "SZ3": mf.C["sz3"], "ZFP": mf.C["zfp"]}

    def panel(name, zoom, fname, cmap="cividis"):
        x = slices[f"{name}/orig"]
        rng = float(x.max() - x.min())
        vmin, vmax = np.percentile(x, [0.5, 99.5])
        if zoom is None:  # pick the window with the most structure (largest gradient energy)
            g = np.abs(np.diff(x, axis=0))[:, :-1] + np.abs(np.diff(x, axis=1))[:-1, :]
            w = x.shape[0] // 5
            best = max(((g[r:r + w, c:c + w].sum(), r, c) for r in range(0, x.shape[0] - w, w // 4) for c in range(0, x.shape[1] - w, w // 4)))
            zoom = ((best[1], best[1] + w), (best[2], best[2] + w))
        (r0, r1), (c0, c1) = zoom
        fig, axes = plt.subplots(3, 4, figsize=(7.4, 6.0), gridspec_kw={"hspace": 0.12, "wspace": 0.04})
        heads = ["original"] + order[:3]  # ZFP cannot reach these rates (see demo_results.json)
        for j, k in enumerate(heads):
            y = x if k == "original" else slices[f"{name}/{k}"]
            axes[0, j].imshow(y, cmap=cmap, vmin=vmin, vmax=vmax, origin="lower")
            axes[0, j].add_patch(plt.Rectangle((c0, r0), c1 - c0, r1 - r0, fill=False, ec=INK, lw=0.6))
            axes[1, j].imshow(y[r0:r1, c0:c1], cmap=cmap, vmin=vmin, vmax=vmax, origin="lower", interpolation="nearest")
            if k == "original":
                t = f"{x.size * 32 / 8 / 1e6:.1f} MB, 32 bpv"
                axes[2, j].text(0.5, 0.5, "absolute error\n÷ range\n(log scale)", ha="center", va="center", color=MUTED, fontsize=7, transform=axes[2, j].transAxes)
                axes[0, j].set_title(k, color=INK, fontsize=8, loc="left")
            else:
                r = res[name]["runs"][k]
                e = np.abs(y.astype(np.float64) - x) / rng
                im = axes[2, j].imshow(np.maximum(e, 1e-6), cmap="magma", norm=LogNorm(1e-5, 1e-1), origin="lower")
                axes[0, j].set_title(f"{k}", color=col[k], fontsize=8, loc="left", weight="bold" if k == "tc-auto" else None)
                t = f"{r['bpv']:.4f} bpv · {r['psnr']:.1f} dB"
            axes[0, j].text(0, -0.09, t, transform=axes[0, j].transAxes, fontsize=6.3, color=MUTED, va="top")
        for a in axes.flat:
            a.set_xticks([]); a.set_yticks([]); a.grid(False)
            for s in a.spines.values():
                s.set_color(mf.LINEHI)
        axes[2, 0].axis("off")
        cb = fig.colorbar(im, ax=axes[2, 1:], fraction=0.02, pad=0.01)
        cb.outline.set_visible(False)
        cb.ax.tick_params(labelsize=6)
        mf.save(fig, fname)

    panel("cesm-TS", None, "fig_demo_cesm")
    panel("miranda-density", ((30, 94), (30, 94)), "fig_demo_miranda", cmap="inferno")

    # pointwise-bound demo: distribution of error / b
    fig, axes = plt.subplots(2, 2, figsize=(3.6, 3.3), sharey=True, sharex=True)
    axes = axes.ravel()
    bins = np.linspace(-1.1, 1.1, 89)
    for a, k in zip(axes, order):
        e = slices[f"bound/{k}"].ravel()
        r = res["cesm-TS-maxabs-1e-3"]["runs"][k]
        a.hist(e, bins=bins, color=col[k], log=True)
        a.axvspan(-1.1, -1, color=mf.SURFACE); a.axvspan(1, 1.1, color=mf.SURFACE)
        a.axvline(-1, color=INK, lw=0.6, ls="--"); a.axvline(1, color=INK, lw=0.6, ls="--")
        a.set_title(k, loc="left", color=col[k], fontsize=8)
        a.text(0.02, 0.97, f"{r['bpv']:.3f} bpv\nmax |e|/b = {r['max_rel'] / 1e-3:.3f}", transform=a.transAxes,
               va="top", fontsize=6.3, color=INK)
        a.set_xlabel("error / b", fontsize=7)
        a.tick_params(labelsize=6)
    axes[0].set_ylabel("cells", fontsize=7); axes[2].set_ylabel("cells", fontsize=7)
    fig.tight_layout()
    mf.save(fig, "fig_demo_bound")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--plot":
        s = np.load(sys.argv[2])
        plot(json.load(open(os.path.join(HERE, "demo_results.json"))), {k: s[k] for k in s.files})
    else:
        main()
