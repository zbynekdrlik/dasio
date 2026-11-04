# dante-asio: a Dante ASIO driver for Windows, design (MVP)

Status: draft, 2026-10-29. Owner: the decisions marked "owner" below; the rest are Claude's calls, marked "my call".

## Intent

The FOH Ableton PC runs two Ableton Live instances. Today each one reaches Dante through
Ableton → a VB-Matrix virtual ASIO device → VB-Matrix → the Yamaha AIC128-D card's ASIO → Dante.
Every hop adds its own buffer, and the total latency is unacceptable (owner).

The bridge is our own Dante device on Windows that a DAW opens as an ASIO driver:
Ableton → bridge → Dante, straight on the PC's network card, at the lowest latency the PC
holds. It reuses what iemmixer Gen 3 proved on Linux: our own Dante stack (`iem-dante`:
discovery, ARC, the media flows, the clock from dantesync), the realtime discipline, and
the measured timing method.

## Goals

- **One bridge = one Dante device + one ASIO driver** a DAW selects by name (owner).
- **Several bridges at once** on one Windows PC, each its own Dante device; on ablfoh two,
  one per Ableton (owner).
- **A buffer per bridge**, set on its own, so a DAW that cannot keep up at a low buffer gets
  a larger one without slowing the other (owner).
- **A tray icon** per running bridge for the basic settings: the channel counts (RX and TX),
  the buffer, the Dante device name, like our other Rust apps (owner).
- **Low-level, realtime network access** on Windows, the Windows counterpart of the Gen 3
  box's tuning and of dantesync's Windows work (owner).
- **The lowest latency the PC holds,** measured, not assumed (owner: "co najnižšie dosiahnuteľná").

## Non-goals (MVP)

- No routing UI: subscriptions are made in Dante Controller, as with any Dante device.
- No sample-rate conversion: 96 kHz only, the site's rate (my call; 48 kHz later if needed).
- No AES67, no Dante Domain Manager, no redundancy (secondary network).
- No macOS or Linux build of the ASIO side.

## Anti-goals

- Never disturb the other Dante devices on the network (the iemmixer Dante rule): the bridge
  changes only its own device.
- Never a second PTP owner: dantesync stays the PC's only clock (as on the Gen 3 box).
- A crashed or closed Ableton must not take the bridge's Dante device down or glitch the other bridge.

## The use on ablfoh (owner, 2026-10-29)

Both Ableton instances replace a hardware mixing console and mix live and recorded sound:

- **Band Ableton** (large, complex): the band's microphones arrive over Dante, it mixes them
  with the backing tracks and sends its mix out over Dante.
- **Master Ableton** (simple, a basic console): takes the band mix plus the preacher's handheld
  mics, the CG sound and similar, does the basic mixing and sends it out; it can play tracks too.

So both are **live round trips** (Dante in → Ableton → Dante out) and they are **in series**:
mic → band Ableton → master Ableton → PA. The latency that matters is the sum of both bridges'
round trips plus the hop between them, so RX and TX both sit on the critical path (P and W
matter on both bridges), and the band → master hop is a Dante flow between two bridges on the
same PC.

**Channel counts (my call):** each bridge configurable up to 64 RX and 64 TX in the MVP's tray
(today each Ableton opens a VB-Matrix 64-channel virtual ASIO device); the MVP's first proof
runs 8/8, then the counts the two sets actually use.

## Decisions

- **The PC (owner, 2026-10-29):** ablfoh. Dante Virtual Soundcard is not used there and may
  be uninstalled. More IP addresses on the network card may be used for the bridges.
- **Language:** Rust (owner).
- **Two processes per bridge (my call):**
  - `dante-asio` service process: the Dante device (discovery, ARC, CMC, RX and TX flows,
    the clock), one per bridge instance, started per instance with its own config.
  - The ASIO driver DLL (an in-process COM server the DAW loads): only the audio hand-off. It
    talks to its instance's process through shared memory rings (lock-free, preallocated) and
    one event per buffer period. A DAW crash leaves the Dante device running.
- **Instance identity (my call):** each instance has its own IP address on the Dante NIC
  (owner allowed), its own Dante name, its own ASIO driver registration (its own CLSID and name
  under `HKLM\SOFTWARE\ASIO`, e.g. "Dante ASIO 1", "Dante ASIO 2"). Dante's fixed ports
  per device then never collide.
- **Clock (my call):** read from dantesync as `iem-dante::clock` does on Linux (dantesync's time
  server for the date offset, the OS clock for the rate); its Windows equivalent of
  `CLOCK_MONOTONIC` is checked first (task 1 of the plan).
- **Timing:** the ASIO buffer is the DAW's period B; the Dante side uses the Gen 3 model
  D = B + P + W (P the RX playout, W the work margin), all per instance in its config. Starting
  point B 32, P and W measured on ablfoh as on the box.
- **Realtime on Windows (my call; each proven by measurement before it stays):** MMCSS "Pro
  Audio" for the media threads, a high-resolution waitable timer, CPU affinity for the media
  threads, the NIC's interrupts and RSS away from them, interrupt moderation off (already off on
  ablfoh), plain UDP sockets first; Registered I/O (RIO) or XDP for Windows only if the
  measurement shows the socket path is the limit.
- **Config:** a TOML file per instance (the device config's shape from Gen 3) that the tray
  icon edits; a change that needs it restarts the instance's process.
- **Repository (my call):** a new private GitHub repository `zbynekdrlik/dasio` with its
  own CI (Windows runner builds the DLL and the process), later its own Claude session (owner).
  `iem-dante` is taken from iemmixer as a pinned git dependency at first, or copied, the plan
  decides.

## MVP scope and proof

1. One bridge, 8 TX and 8 RX (my call; the tray sets up to 64 each), on ablfoh, opened by one Ableton.
2. Visible and routable in Dante Controller; a subscription from and to the IEM rack or the
   FOH console works.
3. A loop test through Dante measuring the round trip, as `gen3-loop` does, compared with today's
   path through VB-Matrix and the AIC128-D.
4. One hour clean: no ASIO buffer missed, no late or missing packet.
5. Then the second bridge at the same time, the second Ableton, the tray icon's settings.

## Left to the builder

The ASIO COM layer details, the shared-memory layout, the installer shape, the tray library,
which parts of `iem-dante` need a Windows path (sockets, threads, the clock).

## Calls after the builder check (my calls, 2026-10-29)

- **Work on ablfoh:** it is the bridge's test PC; changes there are the work (the owner guards
  live use). Each tuning step is per instance or undone after its measurement, and today's path
  (VB-Matrix and the AIC128-D) keeps working until the owner switches an Ableton to the bridge.
- **Addresses:** free addresses in the PC's Dante subnet, each checked unused (ARP, ping, Dante
  discovery) before it is added; they live only in the private config, never in the repo.
- **Clock:** dantesync runs on ablfoh today (service running, checked 2026-10-29); the bridge reads
  it, the AIC128-D's role does not change.
- **Sample rate:** checked on ablfoh before the plan; 48 kHz moves into the MVP if the sets run at 48.
- **`iem-dante`:** copied into `dante-asio` at a pinned iemmixer commit and given its Windows
  paths there (the project gets its own session; the copies may drift; merging them back is later work).
- **The latency proof without other devices:** the bridge's own TX subscribed to its own RX (our own
  device, as the box's HIL loop), the DAW inside the loop.

## Owner, 2026-10-29 (second round)

- **Name:** the project is `dante-asio` (not dante-bridge).
- **Facts are read, not asked:** channel counts and routing come from VB-Matrix, the Ableton
  sets and Dante itself. Read from Dante (read-only, netaudio from the dev box): the PC's
  AIC128-D card has 128 TX and 128 RX; 42 TX and 28 RX carry names, 23 RX are subscribed (the
  band's mics, handheld mics, CG, the engineer's in-ear, sync). That is the whole PC's Dante I/O,
  today shared by both Abletons through VB-Matrix; 64/64 per instance covers it. The exact split
  band/master is read from VB-Matrix's routing in the plan's first task.
- **After a passed MVP:** a coordination between our Dante applications (iemmixer's Gen 3 box,
  dante-asio, dantesync and later ones) so an improvement to the Dante stack is made once and used
  by all of them: one shared Dante stack instead of copies. The MVP's copy of `iem-dante` is
  temporary under that plan.

## Cutover note (not the MVP)

Other devices subscribe to the AIC128-D's TX today (the PA feeds, the in-ear rack). Moving an
Ableton to dante-asio means those receivers subscribe to the new device instead, or the new
device takes the card's names (as the Gen 3 switch does). That changes other devices'
subscriptions, so it is the owner's step and is designed after the MVP.

- **Base (owner, 2026-10-29):** dante-asio starts from the Dante stack built in iemmixer
  (`iem-dante`), nothing else. dante-ndi (a directory on the dev box the owner does not know)
  is not a base and not part of the MVP; the later shared-stack coordination names it only if
  it proves to be one of ours.
