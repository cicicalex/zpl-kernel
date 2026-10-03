#!/usr/bin/env python3
"""Check that a pcap contains the exact ARP request the kernel says it sent.

The kernel's boot log claims it put a frame on the wire. This is the check that
does not take its word for it: QEMU writes what actually left the card to a pcap
through `-object filter-dump`, and this reads that file and asserts on the bytes.

Nothing outside the standard library, and nothing is downloaded.

    python tools/net/check_arp_pcap.py <file.pcap> [--mac 52:54:00:12:34:56]

What it asserts, in order, and it stops at the first thing that is wrong:

  * the file is a pcap, little- or big-endian, with Ethernet link type;
  * exactly one frame is in it -- or two, with `--expect-reply`, when a
    responder answered -- of exactly 60 bytes -- the Ethernet minimum the
    kernel builds to itself rather than leaving to the card to pad;
  * destination is broadcast, source is the MAC the card reported;
  * ethertype is 0x0806, ARP;
  * the ARP header is a request (opcode 1) for IPv4 over Ethernet;
  * the sender and target addresses are the ones the kernel names in its log;
  * the padding is zero -- a frame padded with whatever was in the buffer would
    put kernel memory on the wire, eighteen bytes of it per frame.

Exit 0 when every one of those holds, 1 otherwise.
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

PCAP_MAGIC_LE = 0xA1B2C3D4
PCAP_MAGIC_BE = 0xD4C3B2A1
LINKTYPE_ETHERNET = 1

ETHERTYPE_ARP = 0x0806
ARP_HTYPE_ETHERNET = 1
ARP_PTYPE_IPV4 = 0x0800
ARP_OP_REQUEST = 1

EXPECTED_LEN = 60
EXPECTED_SENDER_IP = (10, 0, 2, 15)
EXPECTED_TARGET_IP = (10, 0, 2, 2)


class CheckFailed(Exception):
    """One assertion did not hold. The message says which."""


def parse_mac(text: str) -> bytes:
    parts = text.split(":")
    if len(parts) != 6:
        raise argparse.ArgumentTypeError(f"not a MAC address: {text}")
    try:
        return bytes(int(p, 16) for p in parts)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"not a MAC address: {text}") from exc


def show_mac(raw: bytes) -> str:
    return ":".join(f"{b:02x}" for b in raw)


def read_frames(path: Path):
    """Every frame in the pcap, as bytes. Raises CheckFailed on a bad file."""
    data = path.read_bytes()
    if len(data) < 24:
        raise CheckFailed(
            f"{path.name} is {len(data)} bytes; a pcap global header alone is 24. "
            "Either QEMU wrote nothing or the filter was not attached."
        )
    (magic,) = struct.unpack("<I", data[:4])
    if magic == PCAP_MAGIC_LE:
        endian = "<"
    elif magic == PCAP_MAGIC_BE:
        endian = ">"
    else:
        raise CheckFailed(f"not a pcap: magic is {magic:#010x}")

    linktype = struct.unpack(endian + "I", data[20:24])[0]
    if linktype != LINKTYPE_ETHERNET:
        raise CheckFailed(f"link type is {linktype}, expected Ethernet ({LINKTYPE_ETHERNET})")

    frames = []
    offset = 24
    while offset + 16 <= len(data):
        _ts_sec, _ts_usec, caplen, origlen = struct.unpack(
            endian + "IIII", data[offset:offset + 16]
        )
        offset += 16
        if offset + caplen > len(data):
            raise CheckFailed("the last record claims more bytes than the file holds")
        frames.append((data[offset:offset + caplen], origlen))
        offset += caplen
    return frames


def check_reply(frame: bytes, mac: bytes) -> list[str]:
    """The second frame, when a responder answered. Asserts it is a reply to us."""
    if len(frame) < 42:
        raise CheckFailed(f"the reply is {len(frame)} bytes, too short for an ARP header")
    if frame[0:6] != mac:
        raise CheckFailed(f"the reply is addressed to {show_mac(frame[0:6])}, not to {show_mac(mac)}")
    (ethertype,) = struct.unpack(">H", frame[12:14])
    if ethertype != ETHERTYPE_ARP:
        raise CheckFailed(f"the reply's ethertype is {ethertype:#06x}, expected ARP")
    (opcode,) = struct.unpack(">H", frame[20:22])
    if opcode != 2:
        raise CheckFailed(f"the second frame's opcode is {opcode}, expected 2 (reply)")
    spa = tuple(frame[28:32])
    tpa = tuple(frame[38:42])
    if spa != EXPECTED_TARGET_IP:
        raise CheckFailed(f"the reply comes from {'.'.join(map(str, spa))}, expected "
                          f"{'.'.join(map(str, EXPECTED_TARGET_IP))} -- the address that was asked about")
    if tpa != EXPECTED_SENDER_IP:
        raise CheckFailed(f"the reply is for {'.'.join(map(str, tpa))}, expected "
                          f"{'.'.join(map(str, EXPECTED_SENDER_IP))}")
    return [
        f"  reply        : {show_mac(frame[6:12])} -> {show_mac(frame[0:6])}, "
        f"{'.'.join(map(str, spa))} is at {show_mac(frame[22:28])}",
    ]


def check(path: Path, mac: bytes, expect_reply: bool = False, pairs: int = 1) -> list[str]:
    """Run every assertion. Returns the lines to print on success.

    `pairs` is how many request/reply exchanges the capture should hold. It is
    one for a boot, which asks once; the console build asks again for every key
    pressed, and a capture of that holds one pair per press.
    """
    frames = read_frames(path)
    per_exchange = 2 if expect_reply else 1
    wanted = per_exchange * pairs
    if len(frames) != wanted:
        what = ("a request and a reply" if expect_reply else "the request alone")
        times = "" if pairs == 1 else f", {pairs} times over"
        raise CheckFailed(
            f"{len(frames)} frames in the capture, expected exactly {wanted} ({what}{times}). "
            "More than that means something else is talking; fewer means it did not happen."
        )
    out: list[str] = []
    for exchange in range(pairs):
        out += check_one(frames, exchange * per_exchange, mac, expect_reply, exchange, pairs)
    return out


def check_one(
    frames, base: int, mac: bytes, expect_reply: bool, index: int, pairs: int
) -> list[str]:
    """One exchange, starting at `base` in the capture."""
    frame, origlen = frames[base]
    if origlen != len(frame):
        raise CheckFailed(f"frame was truncated by the capture: {len(frame)} of {origlen} bytes")
    if len(frame) != EXPECTED_LEN:
        raise CheckFailed(f"frame is {len(frame)} bytes, expected {EXPECTED_LEN}")

    dst, src = frame[0:6], frame[6:12]
    if dst != b"\xff" * 6:
        raise CheckFailed(f"destination is {show_mac(dst)}, expected broadcast")
    if src != mac:
        raise CheckFailed(f"source is {show_mac(src)}, expected {show_mac(mac)}")

    (ethertype,) = struct.unpack(">H", frame[12:14])
    if ethertype != ETHERTYPE_ARP:
        raise CheckFailed(f"ethertype is {ethertype:#06x}, expected {ETHERTYPE_ARP:#06x} (ARP)")

    htype, ptype = struct.unpack(">HH", frame[14:18])
    hlen, plen = frame[18], frame[19]
    (opcode,) = struct.unpack(">H", frame[20:22])
    if htype != ARP_HTYPE_ETHERNET:
        raise CheckFailed(f"ARP hardware type is {htype}, expected {ARP_HTYPE_ETHERNET}")
    if ptype != ARP_PTYPE_IPV4:
        raise CheckFailed(f"ARP protocol type is {ptype:#06x}, expected {ARP_PTYPE_IPV4:#06x}")
    if (hlen, plen) != (6, 4):
        raise CheckFailed(f"ARP address lengths are {hlen}/{plen}, expected 6/4")
    if opcode != ARP_OP_REQUEST:
        raise CheckFailed(f"ARP opcode is {opcode}, expected {ARP_OP_REQUEST} (request)")

    sha = frame[22:28]
    spa = tuple(frame[28:32])
    tha = frame[32:38]
    tpa = tuple(frame[38:42])
    if sha != mac:
        raise CheckFailed(f"ARP sender hardware address is {show_mac(sha)}, expected {show_mac(mac)}")
    if spa != EXPECTED_SENDER_IP:
        raise CheckFailed(f"ARP sender IP is {'.'.join(map(str, spa))}, expected "
                          f"{'.'.join(map(str, EXPECTED_SENDER_IP))}")
    if tha != b"\x00" * 6:
        raise CheckFailed(f"ARP target hardware address is {show_mac(tha)}, expected all zero")
    if tpa != EXPECTED_TARGET_IP:
        raise CheckFailed(f"ARP target IP is {'.'.join(map(str, tpa))}, expected "
                          f"{'.'.join(map(str, EXPECTED_TARGET_IP))}")

    padding = frame[42:60]
    if padding != b"\x00" * 18:
        raise CheckFailed(
            "the 18 padding bytes are not zero: " + padding.hex()
            + ". Whatever was in the buffer went on the wire."
        )

    label = "" if pairs == 1 else f" {index + 1} of {pairs}"
    out = [
        f"exchange{label}: the frame is the one the kernel said it sent",
        f"  frames       : {len(frames)}, this one {len(frame)} bytes "
        "(the Ethernet minimum)",
        f"  ethernet     : {show_mac(src)} -> {show_mac(dst)}, ethertype {ethertype:#06x}",
        f"  arp          : request, who has {'.'.join(map(str, tpa))}, "
        f"tell {'.'.join(map(str, spa))}",
        "  padding      : 18 zero bytes, no kernel memory on the wire",
    ]
    if expect_reply:
        out += check_reply(frames[base + 1][0], mac)
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("pcap", type=Path)
    ap.add_argument(
        "--expect-reply",
        action="store_true",
        help="the capture should also hold the reply the responder sent back",
    )
    ap.add_argument(
        "--pairs",
        type=int,
        default=1,
        help="how many request/reply exchanges the capture should hold",
    )
    ap.add_argument(
        "--mac",
        type=parse_mac,
        default=parse_mac("52:54:00:12:34:56"),
        help="the address the card reported; the default is what QEMU gives it",
    )
    args = ap.parse_args()

    if not args.pcap.exists():
        print(f"[E_ARP_001_NO_PCAP] not found: {args.pcap}", file=sys.stderr)
        return 1
    try:
        for line in check(args.pcap, args.mac, args.expect_reply, args.pairs):
            print(line)
    except CheckFailed as exc:
        print(f"[E_ARP_010_MISMATCH] {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
