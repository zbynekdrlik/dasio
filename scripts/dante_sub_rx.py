#!/usr/bin/env python3
"""Subscribe (or clear) RX channels on ONE receiver by TX name@device,
without querying the transmitter (dasio#207).

usage: dante_sub_rx.py <rx-device> <rxnum>=<txname>@<txdevice> ... | <rxnum>=-
       (`-` removes the subscription)

netaudio's `subscription add` looks the TX channel up on the transmitter and
fails for a device it cannot query (the Yamaha card). This sends the
subscription to our own receiver only. Run it with netaudio's interpreter on
the dev box:

    /opt/pipx/venvs/netaudio/bin/python scripts/dante_sub_rx.py <rx-device> 1=free 01@Card

netaudio is imported only inside `run`, so the argument parsing and the
result reading can be tested where netaudio is not installed (CI).

Exit code: 0 when every wanted channel is verified by a fresh readback or was
already as wanted; 1 on any failure, unconfirmed channel or channel the
result does not mention.
"""

import sys


class UsageError(ValueError):
    """A bad `<rxnum>=...` argument."""


def parse_pairs(args):
    """`['1=name@dev', '2=-']` -> `{1: ('name', 'dev'), 2: None}`."""
    if not args:
        raise UsageError("no <rxnum>=<txname>@<txdevice> or <rxnum>=- given")
    desired = {}
    for arg in args:
        num_text, eq, src = arg.partition("=")
        if not eq:
            raise UsageError(f"{arg!r}: expected <rxnum>=<txname>@<txdevice> or <rxnum>=-")
        try:
            num = int(num_text)
        except ValueError:
            raise UsageError(f"{arg!r}: the RX channel number {num_text!r} is not a whole number") from None
        if num < 1:
            raise UsageError(f"{arg!r}: RX channels count from 1")
        if num in desired:
            raise UsageError(f"{arg!r}: RX channel {num} given twice")
        if src == "-":
            desired[num] = None
            continue
        name, at, device = src.rpartition("@")
        if not at or not name.strip() or not device.strip():
            raise UsageError(f"{arg!r}: expected <txname>@<txdevice> after '=', or '-'")
        desired[num] = (name, device)
    return desired


def describe(result, desired):
    """The result's own fields as printable lines, and the exit code.

    With `verify=True` netaudio fills `verified` (fresh readback matched),
    `unchanged` (already as wanted), `failures` and `unconfirmed`; `sent` is
    filled only when it does not verify. A wanted channel that no field
    mentions counts as a failure: the result must account for every one.
    """
    fields = ("verified", "unchanged", "sent", "unconfirmed", "failures")
    lines = [f"{f}: {dict(getattr(result, f, None) or {})}" for f in fields]
    done = set(result.verified) | set(result.unchanged) | set(result.sent)
    bad = set(result.failures) | set(result.unconfirmed)
    missing = sorted(set(desired) - done - bad)
    if missing:
        lines.append(f"not in the result: {missing}")
    return lines, 1 if (bad or missing) else 0


async def run(application, devices, rx_name, pairs):
    from netaudio.cli_support.execution import populate_devices
    from netaudio.cli_support.selection import match_device_identifier, select_device
    from netaudio.dante.subscription_operations import reconcile_receiver_subscriptions

    desired = parse_pairs(pairs)
    matches = match_device_identifier(devices, rx_name)
    if not matches:
        raise SystemExit(f"RX device {rx_name} not found")
    [(_, rx)] = select_device(matches)
    await populate_devices(application, [rx])
    print(
        "before:",
        {
            n: (rx.rx_channels[n].name, getattr(rx.rx_channels[n], "tx_channel_name", None))
            for n in desired
            if n in rx.rx_channels
        },
    )
    result = await reconcile_receiver_subscriptions(application, rx, desired, verify=True)
    lines, code = describe(result, desired)
    print("\n".join(lines))
    return code


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 2
    try:
        parse_pairs(argv[2:])
    except UsageError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    from netaudio.cli_support.execution import run_command

    rc = run_command(run, argv[1], argv[2:], populate_controls=False, device_identifiers=(argv[1],))
    return rc or 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
