# Decoder in the stream (TCS1)

A tang-compress TCG3 stream can carry the program that decodes it. The
receiver checks the program, compiles it to WGSL and to the CPU interpreter,
and caches the result. Plain TCG1 and TCG3 streams decode exactly as before.

## What the program covers

The coding path is the part of TCG3 decoding that is specific to the
entropy coder. It is now built as `tang-expr` integer graphs in
`src/irdec.rs`:

- **Static residual decoder.**
  - Inputs: rANS state, word position, lane bound, and the context
    (`sig`, `e`, `cls`, `sctx`, `off_init`).
  - Steps: significance bit, the magnitude unary loop over the `MAG0` models,
    the Elias-gamma escape, two modelled low bits, raw bits in chunks of at
    most 16, then the sign.
  - Outputs: state, position, and the residual.
- **Learned residual decoder.**
  - Inputs: the same lane state, plus the net's 5 outputs.
  - Steps: σ-table mixture weight, the 2-logistic integer CDF (`zval` and
    `sig16` read tables from `mt`), the zero flag, the sign, the exp-Golomb
    bucket with carried tails, and bisection for the mantissa.
- **MLP (optional).** The learned model's forward pass, with the weights as
  graph constants (`Net32::graph`).

The rest of decoding stays fixed and is not part of the program:
neighbourhood contexts, prediction, dequantization, the lifting transform
and outliers.

These graphs are also the built-in decoder. By default the WGSL kernel's
`RES_BEGIN…RES_END` region is generated from them. `TC_GPU_HANDRES=1`
restores the hand-written version for A/B runs. On the CPU, native Rust
remains the default; `TC_CPU_IRRES=1` routes decoding through the
interpreter instead.

The graphs are pure, so an `if` becomes a predicate. A decision taken under
a false predicate still computes its rANS step, but keeps the old state; its
word fetch either reads in bounds or returns 0. Each loop folds its predicate
into its condition, so it runs exactly as many iterations as the
hand-written loop.

## Container format

```
TCS1  varint(len)  program[len]  [pad to even]  TCG3 stream
program = "TCDP" 0x01 flags { varint(len) TIR1-graph }*
flags: bit0 static residual, bit1 learned residual, bit2 MLP
```

- **Missing residual sections.** If a residual section is absent, the
  receiver uses its built-in graph for that role.
- **Alignment.** The pad byte keeps the inner stream on a 16-bit word
  boundary. `parse3` shifts every lane, tile and block offset by the header
  length, so the existing CPU and GPU decoders index the outer buffer
  unchanged.
- **Learned streams with only a fingerprint.** Normally the decoder must
  find a registered model matching that fingerprint. If the stream ships its
  MLP, an unregistered fingerprint gets a placeholder net instead, and the
  shipped graph runs. Such a stream needs no model registry
  (`shipped_mlp_needs_no_registry`).

### Graph wire format (`TIR1`, in tang-expr `src/int/wire.rs`)

- **Loop table first.** For each loop: its parent, its trip cap, the number
  of states, the init ids, the condition id, and the next-state ids.
- **Then the nodes in id order.** Each node is a tag byte followed by its
  fields. Operands are encoded as `self − operand`, which is always ≥ 1.
- **Then the output ids.**

Buffers are not named on the wire. The reader supplies the names.

## Verifier rules

The verifier is `IntGraph::from_bytes` plus the role ABIs in
`shipdec::compile`. It rejects:

1. Truncation, trailing bytes, malformed varints, unknown tags or ops.
2. Any operand that is not an earlier node. This makes the graph a DAG by
   construction.
3. Type errors. The checks are the builder's own: arithmetic on bool,
   mismatched operands, a non-`u32` load index, and so on.
4. Scope errors:
   - a node that mixes states of unrelated loops;
   - a loop body that uses a sibling loop's values;
   - a loop init that does not come from the enclosing scope.
5. Malformed loops:
   - a missing or duplicate state node;
   - a loop result that appears before the loop's body;
   - a loop whose parent is not an earlier loop.
6. ABI mismatches. Params must match the role's count and types, buffers
   must match exactly (`mt`, `stream`; none for the MLP), and outputs must be
   top-level values of the role's types.
7. Resource limits:

   | limit | residual roles | MLP |
   |---|---|---|
   | nodes | 2^14 | 2^16 |
   | loops | 16 | 0 |
   | nesting depth | 2 | – |
   | trip cap per loop | 64 | – |
   | worst-case work (Σ nodes × enclosing caps) | 2^16 | 2^16 |
   | program size | 1 MiB | 1 MiB |

What the graph can do is fixed by its op set. That set has integer
arithmetic only; there is no float, store, atomic, barrier, call or I/O.
Loads are read-only, and both backends bounds-check them so that an
out-of-bounds load reads 0: the interpreter does it, and
`to_wgsl_body_guarded` guards with `arrayLength`. Every loop carries a trip
cap, and both backends count trips the same way. A verified graph is
therefore a total, terminating function of its parameters and the two
buffers.

## Threat model

The attacker controls the whole stream, including the program.

- **Memory safety.** Program bytes never index host memory directly:
  - registers are bounded by the node count;
  - loads are clamped to read 0 out of bounds;
  - buffer names come from the host, so no attacker text reaches the WGSL
    source.

  Every identifier the lowering emits is generated by the host (`t{id}`,
  `s{l}_{k}`, `c{l}`), and every literal is a typed integer.
- **Denial of service.**
  - Per residual, work is capped at 2^16 node evaluations.
  - Compile cost is bounded by the node limit.
  - Inlined expressions nest at most 32 deep, because the lowerings recurse
    on them.

  A hostile stream can still make one decode slow, by up to about 2^16× per
  coefficient. A host that decodes untrusted streams should apply its own
  time budget or GPU watchdog. A budget that tracks the stream's coefficient
  count would be a good follow-up.
- **Wrong output.** A valid but different program decodes to different
  numbers. That is the point of shipping a decoder, and it is not a safety
  issue. The integrity of the field rests on the stream as a whole: sign or
  hash the container if that matters. The built-in decoder can always be
  forced by stripping the TCS1 header.
- **Cache poisoning.** The cache is keyed by FNV-1a 64 of the program bytes,
  and a hit is confirmed by comparing the full bytes. A hash collision
  therefore never selects another program's compiled code. GPU pipelines are
  keyed by (net, program hash, program size) inside one `GpuDecoder`. That
  key does not compare bytes. A crafted FNV collision with the same size
  could reuse another program's pipeline, which yields wrong output only;
  hashing with BLAKE3 would close this.
- **Cross-backend divergence.** Byte-identical output across CPU and GPU is
  guaranteed for in-bounds programs. A hostile program that reads out of
  bounds also gets 0 on both backends. Buffer lengths differ between them
  (the GPU `mt` holds the whole metadata block), however, so an
  out-of-bounds index on one backend may be in bounds on the other. Such a
  program may decode differently on CPU and GPU, but it is still safe.

## Results (M4, 2026-10-20)

**Gate: met.** A shipped-decoder stream decodes byte-identically to the
built-in decoder on every path tested:

- native CPU (interpreter);
- native GPU;
- the browser, through WebGPU and through WASM on the CPU: 25/25 streams in
  headless Chrome, including 10 TCS1 streams.

Tests: `shipdec::tests::*`, `irdec::tests::*`, `gpudec::tests::gpu_matches_cpu`.

### NYX 512³, p80, 8 lanes, block 32

GPU kernel throughput, best of 5, two interleaved rounds:

| model | IR (GB/s) | hand-written (GB/s) | shipped (GB/s) | identical |
|---|---|---|---|---|
| static | 1.720 | 1.750 | 1.734 / 1.774 | all |
| pre3v4_16 | 1.093 / 1.023 | 1.013 / 0.941 | 1.080 / 1.103 | all |
| pre3v4_32x16 | 0.842 / 0.836 | 0.794 / 0.812 | 0.830 / 0.849 | all |

- **Static streams:** the IR-generated decoder is within 2% of the
  hand-written kernel.
- **Learned streams:** the IR decoder is 3–8% faster, because the
  predicated form drops the early-return divergence.
- **Guarded loads:** they cost nothing measurable.

Bytes added and compile+cache time:

| stream | stream bytes | program bytes | verify + lower | cache hit | GPU pipeline cold | GPU pipeline hit |
|---|---|---|---|---|---|---|
| static | 981,742 | 3,994 (0.41%) | 0.45 ms | 4 µs | 0.16 s | <1 µs |
| pre3v4_16 + MLP | 959,870 | 10,508 (1.09%) | 0.92 ms | 11 µs | 0.45 s | <1 µs |
| pre3v4_32x16 + MLP | 945,638 | 21,468 (2.27%) | 1.68 ms | 24 µs | 0.69 s | <1 µs |

The cold GPU pipeline time is dominated by shader compilation of the whole
kernel module.

CPU decode of the shipped streams through the interpreter, 16 threads:

| stream | throughput (GB/s) |
|---|---|
| static | 0.103 |
| pre3v4_16 | 0.0139 |
| pre3v4_32x16 | 0.0068 |

All three are byte-identical. Native Rust stays the default for built-in
streams. A compiled CPU backend (`to_rust` and a cache, or a JIT) is the
follow-up.

### Reproduce

```
gpubench encode3 nyx.f32 512x512x512 --bricks 32 --targets p80 --l3 8 --model3 pretrained:M.tcm --out S.tcg3
gpubench ship3 S.tcg3 [--mlp] pretrained:M.tcm        # writes S.tcs3, prints bytes and times
gpubench gpu3 S.tcg3 S.tcs3 pretrained:M.tcm          # TC_GPU_HANDRES=1 runs the hand-written A/B
gpubench cpu3 S.tcs3 pretrained:M.tcm --threads 16
```
