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

  Names are the device's RX channel names as it lists them now: a Dante
  rename (dcontroller) kept in state.toml wins over device.toml's `rx`
  (`miss_watch::rx_names_now`; before 0.1.180 a rename turned the whole watch
  off at the next start). Names are matched once, at the device's start: a
  rename while it runs needs a dasio restart. 1 to 16 per list; a channel may
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
  24-bit step), `peak_dbfs` the peak of `e` itself, 20 log10(steps / 2^23)
  to a tenth (the largest of the line's channels, like `db`), `grid` =
  `at % block` of the config's `block`, `since_prev` samples since that channel's previous event,
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
- Reading `peak_dbfs` and `grid` (24.11.2026): an event under about -50
  dBFS peak is a step nobody hears, whatever its `db`. The host's misses
  share ONE `grid` (see above): no need to compute `at % block` by hand.
  `grid` uses the config's `block`, so with `block = 64` under a host at
  128 the host's misses still share one value (the host's blocks are a
  multiple of 64 apart). The carried peak is exact
  (`ProgramJump.steps`, pinned in the splice test; derived with a Python
  model of the same integer signal).
- Measured (live run 24.11.2026): quiet program material (floor -84 to -94
  dBFS) fired 24-32 dB events at `K = 16` with scattered `grid` values,
  while a signal start on a silent channel reads 70-72 dB on both channels
  at once.
- The loop check (`miss_watch/looped.rs`, 0.1.164, on the `program`
  channels): per sample, the run of samples exactly equal to the sample 1,
  2 and 4 config blocks before. A run of at least one lag less 4 samples in
  which at least one sample in 16 differs from the one before (`varied`:
  silence with two stray steps one lag apart repeats too, with two changes;
  so a ring that is mostly digital silence or one held value is not told)
  is WARN `a watched program output looped` (`channels`,
  `at` = the first repeated sample, `lag`, `samples`, `ms`, `peak_dbfs` =
  the loudest repeated sample, `grid`). Smallest lag first, one report per
  sample; no level floor and no upper length (24-bit audio that moves never
  repeats sample for sample). It reports when the run ENDS; a discontinuity
  drops a run in progress. A bit-exact
  signal whose period divides a lag (at 96 kHz and a block of 64: 375, 750,
  1500, 3000 Hz) is one endless run, reported once when it stops; a 1 kHz sine
  with 32 samples skipped is a 64-sample run at lag 64 (the rtsan test).
- Why it exists (24.11.2026 19:25:17Z): master's outputs replayed the
  card's two half-buffers (256 samples) about 20 times, 55 ms, right after
  a song jump of band Live. The lines then: tone `click samples=1 db=0.3`
  (a phase slip of 256 samples: `2 |sin(256 w / 2)|` = +0.31 dB at 440 Hz;
  one block of 128 would read +5.7 dB, 64 +4.0 dB), the tone window
  `checked` short by 62 blocks, and 10 program events exactly 512 apart
  with ONE peak and ONE ratio (39.5 dB = one more equal step inside each
  256-sample window, so the step recurs every 256 samples and the hold-off
  shows every second). Equal peaks on a sine mean the same samples again.
  The zero/repeat watch stays quiet on such a loop (one block of lag, 8
  blocks at most, channel above -60 dBFS).
- An RX stall is not a host miss (0.1.182, `miss_watch/stall.rs`): every 100 ms
  tick `run.rs` reads each RX flow slot's `gaps` (dstack's own counters, the source
  of its `RX packets missing ... never reached our socket` line) and its flow id
  (`flow_id` of `RxShared::source`, which is `None` after a plan change); `Stall`
  keeps the slots whose count rose in the last 4 ticks. The zero/repeat breaks AND
  the program jumps are held two ticks (`Hold`; dstack confirms a hole after the
  audio shows it, so an item of tick T is judged at T+2 against T-1..T+2); with 2 or
  more sources in the window they log as `a watched output was cut while RX packets
  were missing from several sources: an RX stall ...` (the `broke` line's fields +
  `sources`) and `a watched program audio step came while RX packets were missing
  from several sources ...` instead of `a watched output broke` / `a watched program
  output jumped`, so the event trigger (`tone-trigger.ps1`) matches neither. The
  event is still in the log. Why program jumps too: a zero run's edges are steps
  (a cut to digital silence is a step, see above), so an RX stall can fire the loud-step
  trigger as well. Tone and loop lines are not held or changed.
- The decision is portable and tested (`miss_watch/judge.rs`: `Judge` = `Stall` + two
  `Hold`s, `verdict`, `Due`); `run/watch_log.rs` (Windows-only, outside the mutation
  gate) only logs what `Due` says. A held line comes out 200 ms after its event, so it
  is NOT in log order with tone, loop and 10 s lines: `held_ms` on the line says how
  long it was held (`ago_ms` is read at log time, so it includes it) and `at` (the
  media sample) orders lines. At the end of the device loop `Reader::flush` takes one
  more look at the RX gaps and flows (`Judge::flush`) before judging what is left
  (`held_ms` = the ticks it was held + that look). `run.rs`'s `rx_view` reads gaps
  and flow id as two relaxed reads per slot: a flow change landing between them
  shifts the change by one tick at most.
- What is no loss to `Stall`: a slot's first look (a base, also a slot first seen
  later), a counter that went DOWN (device restart, re-based), and a tick where the
  slot's flow id changed (dstack counts the open holes as gaps when a slot's flow
  changes, so two slots re-planned together would read as a stall and hide a real
  host miss). dstack's `RxCounts` has no flow epoch at the pinned tag (v0.6.226): a
  re-plan to the SAME transmitter inside one tick shows no id change and still
  counts. A break still held when the device loop ends is logged by
  `Reader::flush` (judged by the stall state of the last tick). 27.11.2026
  14:36:13Z (an mbc stall) took a service trace as a `broke` line.
- The event trigger's default (`tone-trigger.ps1 -Match`, 0.1.182) also fires on a
  loud step: `output jumped` with `peak_dbfs` louder than -46 dBFS (-45.x up to a
  full-scale `0.0`; the regex takes `-30` and `0` too) (`kernel-trace.md`).
- The log keeps 4 x 10 MB: pull a service's lines the same day.
- Live's test tone plays only while Live's Settings window is open:
  closing Settings stops it (dasio#207, 25.11.2026: a 200-start run read
  `nothing checked` from the close on, so it proved nothing). Minimizing
  the main window keeps it (Settings is hidden, not closed). Keep Settings
  open for the whole run, read the watch's first tone window before the
  run starts, and switch the tone off in Settings afterwards. Settings
  opens through Options > Settings; find its window by EnumWindows on
  Live's process (a title search also matches AbleSet's "Setlist").
