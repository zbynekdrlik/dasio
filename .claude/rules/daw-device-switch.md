---
paths:
  - "scripts/**"
  - "crates/asio-driver/**"
  - "docs/**"
---

# Switching a DAW onto a dasio instance

A switch is done only when all four steps below are checked.

## 1. Ableton's channel configuration

- When a new ASIO device is chosen, Ableton enables only inputs and outputs 1/2. It shows the set's other routings greyed, and they carry silence.
- Ableton keeps each ASIO device's Input/Output Config (each pair's mono and stereo activator, plus its labels) in its own record in `Preferences.cfg`.
- After the switch, give the new device exactly the previous device's activators and labels. Never "all on": the owner's rule, 3.11.2026.
- Read the previous record from `Preferences.cfg` (the private tools hold a reader).
- Apply it in Settings → Audio → Input/Output Config.
- Then verify by reading the saved `Preferences.cfg` back.

## 2. The DAW's audio while it applies

- Applying a config change in Ableton's Settings freezes its audio for up to about 10 s.
- Late blocks counted in that time are Ableton's, not dasio's. Leave that window out of any measurement.

## 3. Links the old router made between DAWs

- A router such as VB-Matrix also linked the two DAWs directly (one DAW's outputs into the other's inputs) and mixed sums.
- Map every live point of the router to a Dante subscription between our own devices, or to an owner decision.
- After the switch, list every receiver that still subscribes to the old card or router: those receivers are silent now.
- The DAW's inputs move too. On the new device, DAW input N is that device's RX N.
  1. Before the move, save the old device's full RX list (RX number → source channel@device, status), e.g. `netaudio subscription list` or dcontroller `/api/devices`, under `~/.config/dasio/snapshots/`.
  2. Carry over every entry to the same RX number on the new device, with the same source. Unresolved ones are included: the source may only be off right now (e.g. a wireless receiver powered off), and it still needs its subscription.
  3. Read the new device's RX list back and compare it with the saved list, number by number.
- Patches aimed at the old router's channels (e.g. card RX 49 of the VB-Matrix era) feed nothing now.
- Why: the 5.11.2026 move of master onto the AIC128-D carried only the connected entries, so master never got HAND 1 at the 8.11 service (dasio#1).

## 4. Verify with signal

- A subscription in status "Subscribed" proves the transport only. So does a flow's packets in dcontroller's history (`/api/history/{device}`).
- Check the receiving DAW's meter while the sending DAW plays. dasio reads no Ableton itself: ask fohmixer by peer for a read-only meter read of the named track.
- A source nobody can play (a mic with nobody on site) is proven by a substitute source:
  1. Note the RX's current state: its source, or none.
  2. Point that same RX at a channel that carries signal right now, ideally one of our own TX channels. Only that RX changes; no other device's subscriptions are touched. When the RX belongs to a device that is not ours (e.g. the AIC128-D card), the owner OKs the change first, like any subscription change there.
  3. fohmixer reads the DAW track's input meter: above 0 = the path from that RX into the DAW works.
  4. Restore the noted state exactly (a source = re-subscribe it; none = remove the subscription), then read the RX back.
- A track muted in the set passes nothing to the output even when its input meter moves. Report it to the owner; the set is not dasio's.

## Changing an instance's block or channel count

- The shared memory between the driver and the service cannot be resized
  while the DAW holds it.
- Close the DAW's audio device (or the DAW) before a service restart that
  changes `block`, `rx` or `tx`. Otherwise the service refuses to start and
  logs "the DAW still holds the shared memory of the old layout"; SCM keeps
  restarting it until the DAW lets go.
- A restart with the same shape (W, TX latency, margins) is fine with the
  DAW open.

## Restarting a DAW or toggling its audio engine

- After a DAW restart or an Audio Engine toggle, check the instance log for
  `attached=1`, and Ableton's own Start line, before the next step.
- A new driver DLL reaches a DAW only through an install that runs while
  no DAW holds the DLL. `install.ps1 -KeepHeldDriver` leaves the OLD file
  in place, so a later DAW restart alone loads the old DLL again (FOH,
  11.11.2026: one wasted band Live restart).
- To deliver it: arm a script that waits for the DLL to be free and then
  runs `install.ps1` without `-KeepHeldDriver`. It writes `ok` to a done
  file. The DAW owner (fohmixer on FOH) closes the DAW and starts it again
  only after `ok`. Then check the loaded DLL's hash in the DAW process
  against the bundle's (bundles\b0160\deploy-foh-driver.ps1 is the
  pattern).
- An Audio Engine off/on does not load a new driver. Live keeps
  `dasio_driver.dll` loaded with its engine off (FOH, 10.11.2026). COM
  also keeps a loaded DLL per CLSID, so re-registering another file name
  does not help either (dasio#28).
- Quit a DAW only after checking two things: the window title for `*`
  (unsaved changes), and the PC's last input time (someone working).
- At the save prompt, answer No unless the change is one the set needs
  (owner 4.11.2026). Never save after load or recording tests.
- In event mode, nothing is saved for dasio's sake.
- Before a reboot, quit Live through its own quit (No at the save prompt).
  - After a forced restart, Live offers to recover the set from its undo
    store: answer **No**, so that the saved set loads.
  - On mbc, 10.11.2026, "Yes" brought back three track mutes that differed
    from the saved set. The stream lost its sync mic and was ~19 dB quiet
    until they were set back.
