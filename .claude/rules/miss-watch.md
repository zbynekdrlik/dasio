---
paths:
  - "crates/dasio/src/miss_watch.rs"
  - "crates/dasio/src/miss_watch/**"
  - "crates/dasio/src/run/watch_log.rs"
---

# The output watch and its test-tone check (dasio#207)

- Config: the instance folder's `watch.toml`, never a `device.toml` table
  (dtray refuses unknown keys there):

  ```toml
  name = "master"                       # whose outputs, for the log
  block = 128                           # the host's ASIO block, samples
  rx = ["Hand 1 SNV", "CG SNV L"]       # zero/repeat watch; may be left out or [] with `tone`
  tone = ["free 01", "free 02"]         # continuous test tone, checked sample by sample
  ```

  Names are the device's RX channel names, 1 to 16 per list; a channel may
  be in both lists. A config that does not read leaves the watch off and
  the device running (WARN "the output watch is off").
- Log lines to grep: start `watching received outputs for an ASIO host's
  missed buffers (dasio#207)` (fields `rx`, `tone`); zero/repeat `a watched
  output broke like an ASIO host's missed buffer`; tone `a watched test tone
  broke` (`kind` click|level, `at`, `samples`, `db`, `ago_ms`); every 10 s
  per tone channel `a watched test tone over the last 10 s` (`checked`,
  `breaks`, `worst_db`, `amp_min_dbfs`, `amp_max_dbfs`, `hz`; the two
  amplitudes are left out when no level window ended) or `nothing checked`
  (no tone, at or under -80 dBFS, outside 40 Hz..0.45 x rate, or not clean
  enough to fit). WARN `a watched test tone is blind to a repeated or
  skipped host block` beside it: the tone's frequency is wrong, see below.
- Reading a tone break: click `db` = worst residual against the tone
  (-60 dB is the threshold); `samples` = the damaged run: a zero run's own
  length, 1 for a slipped sample or a repeated block (a phase jump), the
  rate (a second) when the tone stopped. Level `db` = the step against the
  running amplitude (0.5 s). A level step with no ramp (a gain switched
  between two samples) is a click of (g - 1) x, not a level break: a level
  break is a step smooth enough to leave every residual under -60 dB. One
  glitch is one break: the channel re-fits (2048 residuals, ~21 ms at
  96 kHz) after every break, so a second glitch inside that window is not
  reported on its own.
- Limits of the check (measured, keep them): the threshold never goes under
  four 24-bit steps (a clean 24-bit sine has residuals up to two), so under
  -66 dBFS a click must be larger than -60 dB to show; under 40 Hz the fit
  is too coarse for the 0.1 dB level check at -80 dBFS; a slipped sample
  shows only when A w > -60 dB (always at 440 Hz, not near a 50 Hz peak).
  It assumes 24-bit RX; a 16-bit sender would break it constantly.
- The tone's frequency: a repeated or skipped host block moves its phase
  by the block's length in periods, a jump of `2 |sin(B w / 2)|` of the
  tone (`block_jump`). When `block` is a whole number of periods (750,
  1500 or 3000 Hz in blocks of 128 at 96 kHz; 1500/3000 Hz in 64 or 32)
  the repeated block IS the tone and nothing shows, the miss this check
  exists for. Under -40 dB (`blind`) the summary WARNs. Use 440 Hz (Live's
  default) or 997 Hz; check `hz x block / rate` is not near a whole number
  for every block size the host runs.
- Mutation gate: every float comparison sits in a small pure helper with a
  boundary test (`above`, `in_band`, `present`, `settles`, `level_break`,
  `blind`, like `loud`); the click end, the one-second cap and the level window's
  position are pinned by sample-exact tests (two clicks RESUME + 2 / + 3
  apart, a stop at 8 kHz fed one sample per block, a level ramp starting on
  a window boundary). Keep `RUN_S` not 1.0: `RUN_S * r` -> `RUN_S + r` is
  then equivalent (the fade test tells 0.5 s from ~1 s).
- The pump-path proof: `tests/rtsan.rs` runs a zero/repeat and a tone
  channel through `Watched::process` under RealtimeSanitizer (the CI
  `rtsan` job). Anything new on the pump side of the watch goes through it.
- Test expectations were derived with a Python model of the exact state
  machine before the Rust existed (no local cargo here): when changing a
  constant, re-derive the exact counts (`checked` = samples - 2050 for a
  clean first fit) the same way, not by pushing to CI.
