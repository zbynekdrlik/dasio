"""The filter's status scripts read the records filteruser.h defines (dasio#27).

rxd.ps1 and rxring.ps1 name each field of DASIO_RXD_STATUS and
DASIO_RX_STATUS and split a reply by each record's own Version. A field
added to a struct without its name (or a layout without its version) would
read the wrong numbers on a PC; this ties the scripts' lists to the header:
the names, in order, and each version's field count, from the struct's
`// Version N` markers.
"""

import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
PACER = ROOT / "drivers" / "ndis-pacer"


def struct_layouts(header, name):
    """Field names in order, and version -> field count, of `name`."""
    m = re.search(r"typedef struct _%s\s*\{(.*?)\}\s*%s\b" % (name, name), header, re.S)
    if not m:
        raise AssertionError(f"no struct {name} in filteruser.h")
    fields = []
    marks = []
    for line in m.group(1).splitlines():
        v = re.search(r"//\s*Version (\d+)", line)
        if v:
            marks.append((int(v.group(1)), len(fields)))
        f = re.match(r"\s*U?LONG64\s+(\w+)\s*;", line)
        if f:
            fields.append(f.group(1))
    const = int(re.search(r"#define %s_VERSION\s+(\d+)" % name, header).group(1))
    layouts = {}
    if marks:
        layouts[marks[0][0] - 1] = marks[0][1]
        for i, (version, _) in enumerate(marks):
            layouts[version] = marks[i + 1][1] if i + 1 < len(marks) else len(fields)
    layouts.setdefault(const, len(fields))
    return fields, layouts, const


def script_layouts(text):
    """The script's names, and version -> field count from its $layouts."""
    names = re.findall(r'"([^"]+)"', re.search(r"^\s*\$names\s*=\s*(.+)$", text, re.M).group(1))
    table = re.search(r"\$layouts\s*=\s*@\{(.+?)\}", text).group(1)
    layouts = {}
    for version, upto in re.findall(r"(\d+)\s*=\s*\$names(?:\[0\.\.(\d+)\])?", table):
        layouts[int(version)] = int(upto) + 1 if upto else len(names)
    return names, layouts


class StatusScripts(unittest.TestCase):
    def setUp(self):
        self.header = (PACER / "driver" / "filteruser.h").read_text(encoding="utf-8")

    def check(self, struct, script):
        fields, want, const = struct_layouts(self.header, struct)
        names, have = script_layouts((PACER / "scripts" / script).read_text(encoding="utf-8"))
        self.assertEqual(names, fields, f"{script}'s names vs {struct}")
        self.assertEqual(have, want, f"{script}'s versions vs {struct}")
        self.assertEqual(max(have), const, f"{script} reads up to {struct}_VERSION")
        # The C_ASSERT next to the struct says the same count.
        size = re.search(r"C_ASSERT\(sizeof\(%s\) == (\d+) \* sizeof\(ULONG64\)\)" % struct, self.header)
        self.assertIsNotNone(size, f"no C_ASSERT on {struct}'s size")
        self.assertEqual(int(size.group(1)), len(fields))

    def test_rxd(self):
        self.check("DASIO_RXD_STATUS", "rxd.ps1")

    def test_rx_ring(self):
        self.check("DASIO_RX_STATUS", "rxring.ps1")

    def test_parser_reads_markers(self):
        header = """
#define DASIO_X_STATUS_VERSION 3
typedef struct _DASIO_X_STATUS
{
    ULONG64 Version;
    LONG64  Cpu;          // a field
    // Version 2: one more
    ULONG64 Two;
    // Version 3:
    ULONG64 Three;
    ULONG64 Three2;
} DASIO_X_STATUS, *PDASIO_X_STATUS;
"""
        fields, layouts, const = struct_layouts(header, "DASIO_X_STATUS")
        self.assertEqual(fields, ["Version", "Cpu", "Two", "Three", "Three2"])
        self.assertEqual(layouts, {1: 2, 2: 3, 3: 5})
        self.assertEqual(const, 3)
        names, have = script_layouts('$names = "A", "B", "C"\n$layouts = @{ 1 = $names[0..1]; 2 = $names }\n')
        self.assertEqual((names, have), (["A", "B", "C"], {1: 2, 2: 3}))


if __name__ == "__main__":
    unittest.main()
