# P1 — time budget on mew (2026-10-18 21:37 UTC snapshot)

## T (Geekbench 5 single-core) — NOT measured, NOT verified

- Geekbench 5.5.1 Linux downloaded from cdn.geekbench.com to `mew:~/hutter/p1/gb5/`. Not run, because:
  - the free build is "Tryout": its `--help` lists no offline or no-upload flag (only `--unlock EMAIL KEY`), so every run uploads to the public Geekbench Browser. Running it needs the user's OK, or a Pro key;
  - it always runs multi-core too, and mew is busy right now: load avg ~10, one python process at ~900% CPU, 3 cmix processes, **swap 8 GB / 8 GB full**, 0.8 GB RAM free. A score taken now would read low.
- I tried to get published figures: browser.geekbench.com returns HTTP 403 to both WebFetch and curl, so I have no citable number. hutter-plan.md's "T ≈ 1,900–2,000" is still unverified.
- **Action:** run `taskset -c 2 ./geekbench5 --cpu` with the user's approval (upload is public and anonymous, no account) while the machine is quiet. Or read the 12900K row on https://browser.geekbench.com/processor-benchmarks in a normal browser.

## Comparability context (measured)

| item | value |
|---|---|
| CPU | i9-12900K, 8P (cpu 0–15, SMT) + 8E (cpu 16–23, 3.9 GHz max) |
| core 6 | **P-core** (physical core 3, max 5.2 GHz, one of the two 5.2 GHz favored cores) |
| scaling driver / governor | intel_pstate active / `powersave` (HWP still boosts; cur freq read 3.2 GHz at the snapshot) |
| turbo | enabled (`no_turbo=0`) |
| package temp | 87 °C at snapshot |
| pinning | hutter-compress=6, hardcore_panini=8, friendly_elbakyan=9 |
| other load | host python ~9 cores at nice 5, 2 extra cmix, frigate/ffmpeg, ~45 home containers, swap full |

All of this shares package power, thermals, L3 and memory bandwidth with core 6. cmix is memory-bound, so the record run is probably **slower than a quiet machine would be**.

## Record compress progress

From `compress.log`: **20.03% at 07:54:38 elapsed** (start 2026-10-18T13:39:31Z).
- linear projection: 7.911 h / 0.2003 = **39.5 h** total compress (cmix's own estimate: 7:54 + 31:35 = 39.5 h)
- decompress ≈ the same (symmetric CM) → ~39.5 h
- actual times will land in `~/hutter/logs/result.txt` (not yet present)

## Allowed hours and headroom (as a function of T)

allowed = 70,000 / T; headroom = 1 − 39.5 / allowed

| T (assumed) | allowed h | headroom |
|---|---|---|
| 1,800 | 38.9 | −1.6% |
| 1,900 | 36.8 | −7.2% |
| 2,000 | 35.0 | −12.9% |

Break-even T = 70,000 / 39.5 ≈ **1,772**.

## Translation to test machines (runtime ∝ 1/T, approximation)

Estimated time on the test machine = 39.5 × T_mew / T_test. This assumes GB5 single-core predicts cmix speed linearly, which may not hold for memory-bound code (DRAM latency doesn't scale with GB5).

| T_mew | on T=1310 (limit 53.4 h) | on T=1427 (limit 49.1 h) |
|---|---|---|
| 1,900 | 57.3 h (over) | 52.6 h (over) |
| 2,000 | 60.3 h (over) | 55.4 h (over) |

The proportional rule makes this the same comparison as the mew table: if mew is over its own limit, it is over on the test machines too.

## Bottom line

On today's contended run, the record reproduction projects to about 39.5 h. That exceeds 70,000/T for any T above about 1,772, which includes the assumed 1,900–2,000. The result says little yet, because both inputs are soft: T is unmeasured, and the run shares a heavily loaded, swapping, 87 °C package. Next: (1) measure T while mew is quiet; (2) compare result.txt to a cmix timing on a quiet, pinned core.

## Update 2026-10-18: measured T from the user

The user looked up the i9-12900K's Geekbench 5 scores: **2,258 single-core**, 16,737 multi-core.

- Allowed on mew: 70,000 / 2,258 = **31.0 h**.
- Projected compress on mew (contended): 39.5 h, so headroom is **−27%**.
- Scaled to the test machines by 1/T: ~68.1 h on the Ryzen 7 (limit 53.4 h), ~62.5 h on the
  i7-1165G7 (limit 49.1 h).

The record was verified within the limit, so this run is almost certainly inflated by load on
mew (full swap, three other cmix runs, a 24 GB eval job, the user's own services). A quiet-machine
timing is needed before any model-size decision. Until then, treat the time budget as having no
known headroom.

## Quiet-machine timing

Run 2026-10-19 10:35–11:00Z on mew. The record's binary (`src/run/cmix`) ran on full enwik9 in a fresh scratch dir, in the hutter-fx2:20.04 image, `--cpuset-cpus=2`, with sibling core 3 left idle. It was stopped at 1.00%. All of our aux jobs (M2 sweep, proxy cmix scoring, m1 pipeline, p2) had drained. The swap gate was dropped with your approval: swap stayed 7/8 GB used with stale pages, while MemAvailable was 37–43 GB.

| | quiet run (core 2) | record run (core 6, 2026-10-18) |
|---|---|---|
| setup (unpack 111 s + 51 s, preprocess, pretrain) | 212 s wall | not timestamped (212 s assumed) |
| cmix elapsed at 1.00% | 20:52 (1252 s) | 19:28 (1168 s) |
| cmix "remaining" at 1% | 34:26:19 | 32:06:32 |
| projected full = elapsed×100 + setup | **34.84 h** | **32.50 h** |
| headroom vs 31.0 h | **−3.84 h** | **−1.50 h** |
| core MHz | 4847–4900 | 4831 (prometheus, cpu6) |
| load1 | 3.7–5.4 | 3.89 |

What was running alongside: the record's own cmix on core 6 (still compressing, ~7.5 GB RSS) and your `hear-heads train` job (100% of one core), plus the usual home services (frigate/ffmpeg, homeassistant, dockerd). No aux jobs of ours were running.

**Caveat: this run wasn't actually "quiet".** The quiet run came out slower than the contended one, at the same ~4.9 GHz. The likeliest cause is that it shared L3 and DRAM bandwidth with the record's cmix, plus hear-heads. Yesterday at 14:02Z the record ran without another multi-GB cmix beside it, since the proxy runs started around 15:06Z. cmix is memory-bound, so a truly quiet number needs the record run finished (or no second cmix). Either way, both measurements put a full compress at 32.5–34.8 h, over the 31.0 h limit for T = 2258. Raw data: mew `~/hutter/p1/quiet_result.txt` (42 samples at 30 s), `quiet_status.log`. `~/hutter/PAUSE` was removed at 2026-10-19T11:00:24Z.

<details><summary>raw quiet_result.txt</summary>

```
# P1 quiet-machine timing (mew, 2026-10-19T10:35:53Z)
conditions at start: met (swap gate dropped by user approval; swapused=7GB, stale pages); aux drained at 2026-10-19T10:35:53Z
--- 2026-10-19T10:35:53Z load: 3.93 2.89 2.08  mem(avail,swapused GB): 43 7
MHz cpu2/3/6:  800.000  800.000  4900.000 
 100  13 hear-heads;99.0   6 cmix;14.3  22 containerd;13.0  14 dockerd; 7.6   0 go2rtc; 4.4   4 frigate.output; 3.3  20 python3; 2.4  19 python3; 2.3   8 gnome-shell; 2.3  22 wireplumber; 2.2   7 ffmpeg;
other running at start:
%CPU   RSS COMMAND
 100 3479944 /home/cam/ribt-hh/target/release/hear-heads train --data train --run /home/cam/runs/hear-heads/h256 --steps 6000 --total 6000 --batch 64
99.0 7513532 ./cmix -e enwik9 enwik9.comp
14.3 126492 /usr/bin/containerd
13.0 310136 /usr/bin/dockerd -H fd:// --containerd=/run/containerd/containerd.sock
 7.6 22076 /usr/local/go2rtc/bin/go2rtc -config=/config/go2rtc_homekit.yml -config=/dev/shm/go2rtc.yaml
 4.4 271308 frigate.output
 3.3 710784 python3 -u -m frigate
 2.4 1209664 python3 -P -m homeassistant --config /config
 2.3 804032 /usr/bin/gnome-shell
 2.3 10452 /usr/bin/wireplumber
 2.2 169716 /usr/lib/ffmpeg/7.0/bin/ffmpeg -hide_banner -loglevel warning -threads 2 -hwaccel_device 0 -hwaccel cuda -hwaccel_output_format cuda -user
 2.1 305284 /home/cam/.opencode/bin/opencode serve --hostname 172.17.0.1 --port 4096
 2.1 158176 /usr/lib/ffmpeg/7.0/bin/ffmpeg -hide_banner -loglevel warning -threads 2 -hwaccel_device 0 -hwaccel cuda -hwaccel_output_format cuda -user
 2.1 160892 /usr/lib/ffmpeg/7.0/bin/ffmpeg -hide_banner -loglevel warning -threads 2 -hwaccel_device 0 -hwaccel cuda -hwaccel_output_format cuda -user

phase marks (wall from docker run, 10 s poll):
  first_unpack_done at +111 s
  second_unpack_done at +162 s
  preprocessing at +192 s
  pretraining_start at +202 s
  pretraining_done at +212 s
  main_progress_start at +212 s
1% line: progress: 1.00%  elapsed: 00:20:52  remaining: 34:26:19 
wall at 1%: 1464 s ; setup (wall to main progress start) = 212 s ; cmix elapsed at 1% = 1252 s ; cmix remaining = 34:26:19
PROJECTED full compress = 1252*100 + 212 s = 34.84 h ; headroom vs 31.0 h = -3.84 h

samples (every 30 s):
--- 2026-10-19T10:36:23Z load: 4.75 3.23 2.23  mem(avail,swapused GB): 40 7 MHz cpu2/3/6:  4800.000  4912.128  4800.000   | 
--- 2026-10-19T10:36:54Z load: 4.63 3.35 2.30  mem(avail,swapused GB): 39 7 MHz cpu2/3/6:  4700.000  800.000  4600.000   | 
--- 2026-10-19T10:37:24Z load: 5.10 3.62 2.43  mem(avail,swapused GB): 39 7 MHz cpu2/3/6:  4900.000  800.000  4900.000   | 
--- 2026-10-19T10:37:54Z load: 5.14 3.80 2.53  mem(avail,swapused GB): 39 7 MHz cpu2/3/6:  4900.000  4900.000  4900.000   | 
--- 2026-10-19T10:38:25Z load: 5.47 4.02 2.65  mem(avail,swapused GB): 38 7 MHz cpu2/3/6:  4900.000  4900.000  4900.000   | 
--- 2026-10-19T10:38:55Z load: 5.76 4.21 2.76  mem(avail,swapused GB): 42 7 MHz cpu2/3/6:  4900.000  4900.000  4900.000   | 
--- 2026-10-19T10:39:26Z load: 5.39 4.29 2.83  mem(avail,swapused GB): 38 7 MHz cpu2/3/6:  4900.000  4874.807  4900.048   | progress: 0.00%  elapsed: 00:00:00 
--- 2026-10-19T10:39:56Z load: 4.99 4.30 2.88  mem(avail,swapused GB): 38 7 MHz cpu2/3/6:  4899.951  4900.000  4900.000   | progress: 0.03%  elapsed: 00:00:36  remaining: 32:55:29 
--- 2026-10-19T10:40:27Z load: 4.78 4.33 2.94  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  4856.055  4900.048   | progress: 0.05%  elapsed: 00:00:59  remaining: 32:58:25 
--- 2026-10-19T10:40:57Z load: 4.57 4.33 2.99  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.048  4900.000  4818.543   | progress: 0.08%  elapsed: 00:01:35  remaining: 33:06:49 
--- 2026-10-19T10:41:28Z load: 4.86 4.44 3.07  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  800.000  4900.000   | progress: 0.10%  elapsed: 00:01:59  remaining: 33:03:53 
--- 2026-10-19T10:41:58Z load: 5.13 4.57 3.15  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4899.950  800.000  4899.951   | progress: 0.13%  elapsed: 00:02:35  remaining: 32:58:13 
--- 2026-10-19T10:42:29Z load: 6.37 4.97 3.34  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  800.000  4900.048   | progress: 0.15%  elapsed: 00:02:58  remaining: 32:58:36 
--- 2026-10-19T10:42:59Z load: 5.74 4.94 3.39  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4957.342  4898.477  4957.405   | progress: 0.18%  elapsed: 00:03:35  remaining: 33:09:24 
--- 2026-10-19T10:43:30Z load: 5.23 4.89 3.42  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  800.000  4900.000   | progress: 0.20%  elapsed: 00:04:00  remaining: 33:18:53 
--- 2026-10-19T10:44:00Z load: 4.97 4.85 3.45  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.097  800.000  4900.000   | progress: 0.23%  elapsed: 00:04:38  remaining: 33:27:42 
--- 2026-10-19T10:44:31Z load: 4.67 4.79 3.47  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.048  800.000  4900.000   | progress: 0.25%  elapsed: 00:05:03  remaining: 33:31:56 
--- 2026-10-19T10:45:01Z load: 4.77 4.79 3.52  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  4900.000  4900.000   | progress: 0.28%  elapsed: 00:05:41  remaining: 33:41:59 
--- 2026-10-19T10:45:32Z load: 5.16 4.86 3.58  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4799.951  4800.000  4800.000   | progress: 0.30%  elapsed: 00:06:06  remaining: 33:48:23 
--- 2026-10-19T10:46:02Z load: 4.84 4.81 3.60  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.049  4900.000  4900.000   | progress: 0.32%  elapsed: 00:06:32  remaining: 33:53:42 
--- 2026-10-19T10:46:33Z load: 5.62 5.02 3.71  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4816.596  800.000  4899.951   | progress: 0.35%  elapsed: 00:07:10  remaining: 33:59:34 
--- 2026-10-19T10:47:03Z load: 5.29 4.99 3.74  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4899.951  4900.000  4900.000   | progress: 0.37%  elapsed: 00:07:35  remaining: 34:03:35 
--- 2026-10-19T10:47:34Z load: 4.96 4.94 3.77  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  800.000  4900.000   | progress: 0.40%  elapsed: 00:08:13  remaining: 34:07:09 
--- 2026-10-19T10:48:04Z load: 4.79 4.90 3.80  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  800.000  4899.901   | progress: 0.42%  elapsed: 00:08:39  remaining: 34:11:08 
--- 2026-10-19T10:48:35Z load: 4.89 4.91 3.84  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4820.768  800.000  4900.049   | progress: 0.45%  elapsed: 00:09:15  remaining: 34:06:00 
--- 2026-10-19T10:49:05Z load: 4.56 4.83 3.85  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4899.950  4899.051  4900.048   | progress: 0.47%  elapsed: 00:09:39  remaining: 34:02:02 
--- 2026-10-19T10:49:36Z load: 4.75 4.84 3.88  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  4899.819  4825.182   | progress: 0.50%  elapsed: 00:10:14  remaining: 33:57:12 
--- 2026-10-19T10:50:07Z load: 5.06 4.90 3.93  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  4900.000  4900.000   | progress: 0.52%  elapsed: 00:10:39  remaining: 33:56:14 
--- 2026-10-19T10:50:37Z load: 4.88 4.88 3.95  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  4900.000  4900.000   | progress: 0.55%  elapsed: 00:11:16  remaining: 33:58:03 
--- 2026-10-19T10:51:08Z load: 5.12 4.94 4.00  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.048  800.000  4900.000   | progress: 0.57%  elapsed: 00:11:42  remaining: 34:00:27 
--- 2026-10-19T10:51:38Z load: 5.21 4.98 4.04  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  4900.000  4900.000   | progress: 0.59%  elapsed: 00:12:07  remaining: 34:02:38 
--- 2026-10-19T10:52:09Z load: 5.58 5.09 4.12  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  800.000  4900.000   | progress: 0.62%  elapsed: 00:12:46  remaining: 34:05:33 
--- 2026-10-19T10:52:39Z load: 5.42 5.11 4.15  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  4900.000  4900.000   | progress: 0.64%  elapsed: 00:13:11  remaining: 34:07:25 
--- 2026-10-19T10:53:10Z load: 4.82 5.00 4.15  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  4900.000  4899.951   | progress: 0.67%  elapsed: 00:13:50  remaining: 34:10:57 
--- 2026-10-19T10:53:40Z load: 4.67 4.96 4.17  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4899.951  4900.000  4900.000   | progress: 0.69%  elapsed: 00:14:16  remaining: 34:13:12 
--- 2026-10-19T10:54:11Z load: 4.23 4.84 4.15  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  800.000  4857.986   | progress: 0.71%  elapsed: 00:14:42  remaining: 34:15:45 
--- 2026-10-19T10:54:41Z load: 4.31 4.81 4.17  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4899.950  800.000  4900.000   | progress: 0.74%  elapsed: 00:15:20  remaining: 34:17:13 
--- 2026-10-19T10:55:12Z load: 4.04 4.70 4.15  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.048  4900.000  4900.000   | progress: 0.76%  elapsed: 00:15:46  remaining: 34:18:05 
--- 2026-10-19T10:55:43Z load: 4.01 4.64 4.15  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  800.000  4900.000   | progress: 0.78%  elapsed: 00:16:11  remaining: 34:19:31 
--- 2026-10-19T10:56:13Z load: 3.87 4.54 4.13  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  4900.000  4900.000   | progress: 0.81%  elapsed: 00:16:50  remaining: 34:21:49 
--- 2026-10-19T10:56:44Z load: 4.05 4.52 4.13  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  4899.179  4899.951   | progress: 0.83%  elapsed: 00:17:16  remaining: 34:22:48 
--- 2026-10-19T10:57:14Z load: 3.83 4.41 4.11  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4899.951  800.000  4900.000   | progress: 0.86%  elapsed: 00:17:55  remaining: 34:25:10 
--- 2026-10-19T10:57:45Z load: 3.95 4.38 4.11  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  4900.000  4900.000   | progress: 0.88%  elapsed: 00:18:20  remaining: 34:25:40 
--- 2026-10-19T10:58:15Z load: 3.79 4.29 4.09  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  4887.990  4900.000   | progress: 0.90%  elapsed: 00:18:46  remaining: 34:26:54 
--- 2026-10-19T10:58:46Z load: 3.52 4.18 4.06  mem(avail,swapused GB): 40 7 MHz cpu2/3/6:  4900.000  800.000  4900.000   | progress: 0.93%  elapsed: 00:19:24  remaining: 34:26:25 
--- 2026-10-19T10:59:16Z load: 4.18 4.26 4.09  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  800.000  4900.000   | progress: 0.95%  elapsed: 00:19:49  remaining: 34:26:14 
--- 2026-10-19T10:59:47Z load: 4.10 4.24 4.08  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4900.000  800.000  4900.000   | progress: 0.98%  elapsed: 00:20:27  remaining: 34:26:38 
--- 2026-10-19T11:00:17Z load: 3.66 4.12 4.05  mem(avail,swapused GB): 37 7 MHz cpu2/3/6:  4846.604  4869.137  4900.000   | progress: 1.00%  elapsed: 00:20:52  remaining: 34:26:19 

## record run (contended) at its 1% mark, from logs/compress.log (read-only)
progress: 1.00%  elapsed: 00:19:28  remaining: 32:06:32 
progress: 1.51%  elapsed: 00:30:12  remaining: 32:49:39 
record container start 2026-10-18T13:39:31Z; cmix elapsed 1%=19:28 -> projected = 1168*100 s + setup(212 s assumed same) = 32.50 h
est. record 1% wall time: 2026-10-18T14:02:31Z; prometheus at that time:
  node_load1 = 3.89
  node_memory_MemAvailable_bytes/1e9 = 43.067154432
  (node_memory_SwapTotal_bytes-node_memory_SwapFree_bytes)/1e9 = 8.589889536
  node_cpu_scaling_frequency_hertz{cpu="6"}/1e6 = 4830.692

PAUSE removed at 2026-10-19T11:00:24Z
```
</details>
