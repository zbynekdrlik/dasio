---
paths:
  - ".cargo/mutants.toml"
  - ".github/workflows/ci.yml"
  - "crates/*/src/**/windows.rs"
---

# The diff-scoped mutation gate (CI `mutation` job, 20 min)

- cargo-mutants mutates `#[cfg(windows)]` code too: on the Linux runner it is
  compiled out, so every such mutant is MISSED. Put Windows glue in its own
  `<module>/windows.rs` and add it to `exclude_globs` in
  `.cargo/mutants.toml` (cpu_sets, dasio#120); never an inline
  `#[cfg(windows)] mod windows { .. }` in a mutated file.
- A `while` walk over a buffer (`at += size`) hangs under its `*=`/`-=`
  mutants and the job is cancelled at 20 min with no log. Bound the loop by
  the most iterations the input allows (`for _ in 0..len.div_ceil(8)`).
  The same for a walk over a mask's bits: `cpus()`'s `while i < 64` ran
  forever under `i += 1` → `i *= 1` and an inverted bit test, and both
  mutation jobs of PR #200 died ("runner has received a shutdown
  signal"); it is `for i in 0..=64` now.
- `a | b` on masks that never overlap is an equivalent mutant (`^`): either
  compute the mask another way (one `range`) or add the function to the
  `replace \| with \^ in (...)` exclusion with the reason.
- Error-path tests assert the whole message: a `contains("size 0")` let a
  `||`-as-`&&` mutant through (it failed one check later, same words).
  Test edge checks at an offset > 0 too (`len - at` as `len + at`).
- The job runs in two shards (`--shard k/2 --sharding round-robin`): one
  runner tests ~14 mutants a minute (141 in 10 min, PR #208), and the tone
  check's diff had 252. Count a large diff's mutants before pushing:
  `cargo mutants --list --in-diff <diff>` only parses, it builds nothing.
  Past ~200 a shard (14 min), add a third rather than raising the timeout.
