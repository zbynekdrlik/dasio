# Hand-off: dante-asio's own session (2026-10-30)

This project was started from an iemmixer session. Work continues here, in this repository's own session. Read this, `CLAUDE.md`, the design and issue #1. Site facts (addresses, real names) stay private: `~/.config/dasio/SITE.md`, tools beside it.

## State

- Device process runs on the FOH PC as instance 1 (scheduled task `dante-asio-1`), ASIO driver registered, shm DACL fix deployed (dev).
- Listening test 2026-10-30: master Ableton on the driver at 64 samples, stage rack on our TX. The owner heard it: plays well, no acoustic problem. Then switched back: master on VB-Matrix, every receiver back on the console card. Today's production path is untouched.
- The test proved: ASIO through shm, RX, TX to a hardware receiver, Dante Controller patching and naming.

## Open, in order

1. **TX stalls of about 5.4 ms** (issue #1, 13:00 finding). Rack's Latency tab: late on about 1 % of packets. Stamps are right (median 118 µs against hardware). Inaudible in the test, but our counters read 0. First make them see it (arrival minus stamp at the send, a histogram), then find the cause: Windows TX thread sleep/timer, CPU shared with RX.
2. **mbc's DVS is refused** (0x0100 refused, its subscriptions status 21). The refusal reason logs at debug only: suspected fpp above `tx_fpp_max = 8`. Log refusals at info.
3. **RX**: Hand 1, 2, 4 and Hand 1 sync do not connect on master-asio-dante. Hand 2: the stage rack answers 0x0311. Hand 1/4 and sync: cause unknown.
4. **Non-Dante links** master has through VB-Matrix (issue #1, 22:45 finding): meeting and strih over VBAN, Chrome, band Ableton feeds, Solo to the card. Owner chose to bridge them so a switch loses nothing.
5. Red Sync in Dante Controller for our devices (we follow dantesync, no PTP of our own): find out what makes it green.
6. Rest of the MVP plan: tray, band-asio-dante, 1 h clean run, latency against VB-Matrix.

## Gotchas learned the hard way

- Live lists ASIO drivers only at start; toggling the driver type does not rescan, and picking MME drops the audio engine until a device is chosen. Restart Live after installing the driver.
- A new device in Live has only 1/2 enabled: enable all pairs in Input and Output Config. Live remembers per device.
- Master Ableton has unsaved work in the set open now? Use File > Save a Copy before quitting; never overwrite master.als without the owner.
- Master runs as another Windows user, non-elevated; the device elevated. Shared objects need the explicit DACL. Live's prefs for master are under that user's profile.
- Check Repro L/R against Repro 0db with VB-Matrix's XML before any switch: a swap plays the speakers at full level.
- Check the receiver's Latency tab, not only routing status. Green routing said nothing about the stalls. Owner cares about how packets flow.
- Windows updates or a replugged USB NIC can leave DVS stamping 61 ms late while PTP looks locked. Restart `dvs.manager`; check latency after every DVS start.

## Rules

- No workarounds for misconfigured transmitters: fix the source PC.
- Never change another device's subscriptions without the owner's approval.
- Do not damage the Ableton or VB-Matrix configuration.
