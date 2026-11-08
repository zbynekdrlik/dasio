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
- The DAW's inputs move too. On the new device, DAW input N is that device's RX N. Carry over every input subscription of the old device by its number, Unresolved ones included.
  - A source that is off during the move (e.g. a wireless receiver powered off) still needs its subscription.
  - The 5.11.2026 move of master onto the AIC128-D carried only the connected ones. Master's "Hand 1" input (card RX 1) stayed empty until the 8.11 service, where HAND 1 never reached master.
  - Patches aimed at the old router's channels, such as card RX 49, feed nothing now.

## 4. Verify with signal

- A subscription in status "Subscribed" proves the transport only. Packets arriving at a card (dcontroller) prove nothing about the DAW either.
- Check the receiving DAW's meter while the sending DAW plays.
- A source nobody can play (a mic with nobody on site) is proven by a substitute source:
  1. Point the same RX at a Dante channel that carries signal.
  2. Read the DAW track's input meter (fohmixer, read only).
  3. Point the RX back.
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
