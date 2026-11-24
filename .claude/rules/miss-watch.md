---
paths:
  - "crates/dasio/src/miss_watch.rs"
  - "crates/dasio/src/miss_watch/**"
  - "crates/dasio/src/run/watch_log.rs"
---

# The output watch, its test-tone check and its program-audio splice detector (dasio#207)

- Config: the instance folder's `watch.toml`, never a `device.toml` table
  (dtray refuses unknown keys there):

  ```toml
  name = "master"                       # whose outputs, for the log
  block = 128                           # the host's ASIO block, samples
  rx = ["Hand 1 SNV", "CG SNV L"]       # zero/repeat watch; may be left out or [] with `tone` or `program`
  tone = ["free 01", "free 02"]         # continuous test tone, checked sample by sample
  program = ["Repro L", "Repro R"]      # real program audio, checked for a splice (no tone needed)
  ```

  Names are the device's RX channel names, 1 to 16 per list; a channel may
  be in all three lists; `program` is optional (old files still read) and at
  least one of the three lists must be non-empty. A config that does not read leaves the watch off and
  the device running (WARN "the output watch is off").
- Log lines to grep: start `watching received outputs for an ASIO host's
  missed buffers (dasio#207)` (fields `rx`, `tone`, `program`); zero/repeat `a watched
  output broke like an ASIO host's missed buffer`; tone `a watched test tone
  broke` (`kind` click|level, `at`, `samples`, `db`, `ago_ms`); every 10 s
  per tone channel `a watched test tone over the last 10 s` (`checked`,
  `breaks`, `worst_db`, `amp_min_dbfs`, `amp_max_dbfs`, `hz`; the two
  amplitudes are left out when no level window ended) or `nothing checked`
  (no tone, at or under -80 dBFS, outside 40 Hz..0.45 x rate, or not clean
  enough to fit; the program channel's one says digital silence). A silent
  channel logs `nothing checked` at its first silent window and then every
  10 min (the 61st, 121st ... window in a row), with `silent_s` = the silent
  windows in a row x 10 (`miss_watch/silence.rs`, portable so the mutation gate runs it: `run` is Windows-only); a window that checked something logs
  its summary as always and restarts the count, so no line for 10 min is
  not a missing line. WARN `a watched test tone is blind to a repeated or
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
  tone (`block_jump`). When `block` is a whole number of periods (at
  96 kHz: 750, 1500 or 3000 Hz in blocks of 128; 1500 or 3000 Hz in 64;
  3000 Hz in 32) the repeated block IS the tone and nothing shows, the
  miss this check exists for. Under -40 dB (`blind`) the summary WARNs.
  Use 440 Hz (Live's default) or 997 Hz; check `hz x block / rate` is not
  near a whole number for every block size the host runs.
- Mutation gate: every float comparison sits in a small pure helper with a
  boundary test (`above`, `in_band`, `present`, `settles`, `level_break`,
  `blind`, like `loud`); the click end, the one-second cap and the level
  window's position are pinned by sample-exact tests (two clicks RESUME + 2
  / + 3 apart, a stop at 8 kHz fed one sample per block, a level ramp
  starting on a window boundary). Keep `RUN_S` not 1.0: `RUN_S * r` -> `RUN_S + r` is
  then equivalent (the fade test tells 0.5 s from ~1 s).
- The pump-path proof: `tests/rtsan.rs` runs a zero/repeat and a tone
  channel through `Watched::process` under RealtimeSanitizer (the CI
  `rtsan` job). Anything new on the pump side of the watch goes through it.
- Test expectations were derived with a Python model of the exact state
  machine before the Rust existed (no local cargo here): when changing a
  constant, re-derive the exact counts (`checked` = samples - 2050 for a
  clean first fit) the same way, not by pushing to CI.
- The program splice detector (`miss_watch/program.rs`, sibling of
  `tone.rs`): master's missed ASIO callback leaves a step between two valid
  waveforms in real audio, neither exact zero nor exact repeat. Per channel
  the 4th difference `h = x - 4x1 + 6x2 - 4x3 + x4`, `e = |h|`; an event is a
  local peak of `e` (+-4 samples, the first of equal) that is `K` times the
  mean `e` of the 256 samples ending 8 before it AND of the 256 starting 8
  after it, and above 1024 24-bit steps (-78 dBFS). Decided 263 samples
  late; 512 samples hold-off per channel. `K = 16` is PROVISIONAL until the
  FOH calibration run (tone on Null L/R and this on Repro L/R together, Live's
  CPU Usage Simulator at 50 %: every tone break needs a program event at the
  same time, a clean stretch none); change it only from that run.
- Its log lines: `a watched program output jumped` (`channels` every
  program channel whose peak is within +-1 sample, `at` the media sample of
  the peak, `db` peak over the larger window mean (the mean at least one
  24-bit step), `since_prev` samples since that channel's previous event,
  `rem` = `since_prev % block`; both absent for a channel's first event);
  every 10 s per program channel `a watched program output over the last
  10 s` (`checked` samples judged, `jumps`, `worst_db` = the largest
  peak-over-window ratio of ANY local peak above the floor, event or not,
  so it shows how near a clean stretch came to `K`, left out with no such
  peak; `floor_dbfs` = mean `e` of the judged samples) or `nothing checked`
  (digital silence for the whole window).
- Reading `rem`: the host's misses are whole blocks apart, so `rem = 0`
  again and again is a master miss; music's false hits (a loud onset into
  a pure tone, a hard cut to silence) have any `rem`. Known limit, tested:
  a clean sine starting from silence IS an event (its later window is as
  quiet as the earlier one); real music keeps `e` raised after an onset.
  A cut to digital silence is a step too.
- Tests for it derived with a Python model of the exact state machine
  (`checked` = samples - 530 for a clean channel; peaks at the splice + 2
  for a 1 kHz sine); the 24-bit RX makes the sliding sums exact. The
  detector's tests are in `miss_watch/program/tests.rs`.
- Reading master's splices (21.11 to 24.11.2026): the host's block grid is
  fixed in the media sample position, so the `at` of the host's splices all
  share one value of `at % <host block>` until the host's audio engine
  restarts; a radio dropout or a musical false hit lands anywhere. Compute
  it when reading (`rem` uses the config's `block`, not the host's).
- The log keeps 4 x 10 MB: pull a service's lines the same day.
