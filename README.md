# dasio

Our own Dante device per instance on Windows, opened by a DAW as an ASIO driver: the DAW
talks to Dante directly, at the lowest latency the PC holds. Several instances run at once,
each with its own Dante name, channels and buffer, set from a tray icon. They share the
PC's own address, each on its own ports (dstack's per-PC manager).

Built on the Dante stack of [iemmixer](https://github.com/zbynekdrlik/iemmixer)
(`dstack`, from [dstack](https://github.com/zbynekdrlik/dstack)). Design: `docs/design/`; plan: `docs/plans/`.

License: GPL-3.0-or-later.
