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

## 4. Verify with signal

- A subscription in status "Subscribed" proves the transport only.
- Check the receiving DAW's meter while the sending DAW plays.
