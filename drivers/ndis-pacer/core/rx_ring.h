/* dasio kernel RX (dasio#27, B23): the filter's decisions for received
 * frames of dasio's own flows, free of any kernel type.
 *
 * Every Dante audio packet goes up through tcpip, WFP and Winsock to
 * dstack's flow sockets, and a stall anywhere on that path is a late
 * packet or a hole. The filter sees each frame first, on the NIC's DPC:
 * a frame whose (destination IPv4, destination UDP port) is in a ring's
 * match table is copied into that ring, a block of memory shared with
 * the dasio process, with its receive time. The NDIS glue
 * (driver/rxring.c) calls these functions under the filter's RX ring
 * lock.
 *
 * The ring (little endian), as the reader in user mode sees it:
 *
 *   header page, PACER_RX_HEADER_BYTES:
 *     0   magic  PACER_RX_MAGIC (the bytes "DRXR")
 *     4   version PACER_RX_VERSION
 *     8   slot count (a power of 2, PACER_RX_MIN_SLOTS..PACER_RX_MAX_SLOTS)
 *     12  slot bytes PACER_RX_SLOT_BYTES
 *     16  data offset PACER_RX_HEADER_BYTES (the first slot)
 *     64  head, 8 bytes: frames published; written only by the filter
 *     128 tail, 8 bytes: frames consumed; written only by the reader
 *     192 wake, 4 bytes: set non-zero by the reader before it waits
 *     256 detached, 4 bytes: 1 once the filter detached the ring (a pause
 *         or detach of the filter, or its close): no frame comes any more,
 *         the reader closes it and opens a new one
 *   then the slots; frame number n is in slot n & (slot count - 1):
 *     0   receive time, 8 bytes, 100 ns interrupt time
 *     8   source IPv4, 4 bytes as on the wire (a.b.c.d)
 *     12  destination IPv4, 4 bytes as on the wire
 *     16  source UDP port, 2 bytes (a number)
 *     18  destination UDP port, 2 bytes
 *     20  payload offset in the frame, 2 bytes
 *     22  payload length, 2 bytes
 *     24  frame length, 2 bytes
 *     26  0, 2 bytes
 *     28  the frame's number n, low 4 bytes
 *     32  the whole Ethernet frame, at most PACER_RX_MAX_FRAME bytes
 *
 * One reader. It reads slots tail .. head - 1, then stores the new tail.
 * Before it waits it stores wake = 1 and reads head again (both
 * sequentially consistent); the filter publishes head (a full barrier),
 * then reads wake, clears it and sets the reader's event: one event per
 * sleep, never a lost one; the event is set when the ring is detached
 * too. The filter keeps its own head and masks every
 * slot index, so no value the reader writes (tail, or anything else in
 * the header) makes it write outside the ring; a tail it cannot believe
 * (ahead of head, or more than a ring behind) reads as a full ring. */
#ifndef RX_RING_H
#define RX_RING_H

#include "pacer_core.h"

#define PACER_RX_MAGIC 0x52585244u /* "DRXR" in memory order */
#define PACER_RX_VERSION 1u
#define PACER_RX_HEADER_BYTES 4096u
#define PACER_RX_SLOT_BYTES 2048u
#define PACER_RX_META_BYTES 32u
#define PACER_RX_MAX_FRAME (PACER_RX_SLOT_BYTES - PACER_RX_META_BYTES)
#define PACER_RX_MIN_SLOTS 64u
#define PACER_RX_MAX_SLOTS 4096u

#define PACER_RX_OFF_MAGIC 0u
#define PACER_RX_OFF_VERSION 4u
#define PACER_RX_OFF_SLOTS 8u
#define PACER_RX_OFF_SLOT_BYTES 12u
#define PACER_RX_OFF_DATA 16u
#define PACER_RX_OFF_HEAD 64u
#define PACER_RX_OFF_TAIL 128u
#define PACER_RX_OFF_WAKE 192u
#define PACER_RX_OFF_DETACHED 256u

/* Entries of one ring's match table at most. */
#define PACER_RX_MAX_MATCHES 16u

/* One received UDP packet's addresses and where its payload is. */
struct pacer_rx_packet {
    uint8_t src_ip[4];
    uint8_t dst_ip[4];
    uint16_t src_port;
    uint16_t dst_port;
    /* From the frame's first byte. */
    uint32_t payload_off;
    uint32_t payload_len;
};

/* Reads the headers of a frame of `frame_len` bytes from its first
 * `avail` bytes (`avail` <= `frame_len`; only those are read). Returns 1
 * and fills `p` for Ethernet II, 0 to 2 VLAN tags (0x8100 / 0x88A8),
 * IPv4 (version 4, header 20..60 bytes), not a fragment (MF clear,
 * offset 0), UDP, with the IP total length inside the frame and the UDP
 * length (at least 8) inside the IP payload; the payload is the UDP
 * length's. 0 for anything else, or when a header lies past `avail`. */
int pacer_rx_parse(const uint8_t *hdr, uint32_t avail, uint32_t frame_len, struct pacer_rx_packet *p);

/* One match entry: a destination IPv4 address (as on the wire) and UDP
 * port. Address 0.0.0.0 matches any unicast destination on that port: the
 * flows sent to this PC's address, but also another host's unicast that a
 * switch floods to this NIC (read the slot's destination, or name the
 * PC's own address instead). A multicast group (224.0.0.0/4) or the
 * broadcast address 255.255.255.255 matches only an entry naming it. */
struct pacer_rx_match_entry {
    uint8_t addr[4];
    uint16_t port;
};

/* 1 when an entry of `table` (`count` entries, at most
 * PACER_RX_MAX_MATCHES are looked at) matches the packet's destination. */
int pacer_rx_match(const struct pacer_rx_match_entry *table, uint32_t count, const struct pacer_rx_packet *p);

/* 1 when `table` can be a ring's match table: at most
 * PACER_RX_MAX_MATCHES entries, none with port 0. */
int pacer_rx_match_valid(const struct pacer_rx_match_entry *table, uint32_t count);

/* The producer's side of one ring. */
struct pacer_rx_ring {
    /* The ring's memory, 8-byte aligned: pacer_rx_ring_bytes(slots). */
    uint8_t *base;
    uint32_t slots;
    /* Frames published (the filter's own copy of head). */
    uint64_t head;
    /* The reader's tail when it was last read. */
    uint64_t tail_seen;
};

/* The bytes of a ring of `slots` slots (header page and slots), or 0
 * when `slots` is not a power of 2 in PACER_RX_MIN_SLOTS..
 * PACER_RX_MAX_SLOTS. */
uint32_t pacer_rx_ring_bytes(uint32_t slots);

/* Zeroes the whole ring at `base` (nothing of the memory's earlier
 * contents reaches the reader), writes its header and sets up `r`.
 * Returns 1, or 0 when `slots` is not valid (nothing written). */
int pacer_rx_ring_init(struct pacer_rx_ring *r, uint8_t *base, uint32_t slots);

/* Tells the reader the ring is detached: the header's detached word
 * becomes 1 (the caller then sets the reader's event). */
void pacer_rx_ring_detach(struct pacer_rx_ring *r);

enum pacer_rx_put_result {
    PACER_RX_DELIVERED = 0,
    /* Every slot holds a frame the reader has not consumed. */
    PACER_RX_FULL = 1,
    /* Longer than PACER_RX_MAX_FRAME, or the caller could not read it
     * (frame NULL). */
    PACER_RX_TOO_BIG = 2,
};

/* Copies one frame of `len` bytes, with the packet's addresses and
 * `time_100ns`, into the next slot and publishes it. `*wake` is set to 1
 * when the reader asked to be woken (the ring's wake was non-zero; it is
 * cleared), else left alone. */
enum pacer_rx_put_result pacer_rx_put(struct pacer_rx_ring *r, const uint8_t *frame, uint32_t len,
                                      const struct pacer_rx_packet *p, uint64_t time_100ns, int *wake);

/* One ring with its match table, mode and counters. */
struct pacer_rx_sink {
    struct pacer_rx_ring ring;
    struct pacer_rx_match_entry match[PACER_RX_MAX_MATCHES];
    uint32_t matches;
    /* 1: steal mode (a frame it matches does not go up to tcpip). */
    uint32_t steal;
    /* Set to 1 by pacer_rx_deliver when the reader asked for a wake; the
     * caller sets the reader's event and clears it. */
    int wake;
    uint64_t matched;
    uint64_t delivered;
    uint64_t full;
    uint64_t stolen;
    uint64_t too_big;
};

/* Replaces `s`'s match table with `count` entries of `table`. Returns 1,
 * or 0 when the table is not valid (pacer_rx_match_valid; nothing
 * changes). */
int pacer_rx_set_match(struct pacer_rx_sink *s, const struct pacer_rx_match_entry *table, uint32_t count);

/* 1 when any of `count` sinks matches the packet. */
int pacer_rx_wanted(struct pacer_rx_sink *const *sinks, uint32_t count, const struct pacer_rx_packet *p);

/* One received frame for `count` sinks: put into every sink whose table
 * matches (each counts it in matched, then in delivered, full or
 * too_big). `frame` holds the whole frame of `len` bytes, or is NULL when
 * the caller could not read it (counted as too big). Returns 1 when the
 * frame must not go up to tcpip: at least one sink matched, every
 * matching sink steals, and none counted it too big (each matching sink
 * then counts it in stolen; a full stealing ring drops it). 0: it goes
 * up as before. */
int pacer_rx_deliver(struct pacer_rx_sink *const *sinks, uint32_t count, const struct pacer_rx_packet *p,
                     const uint8_t *frame, uint32_t len, uint64_t time_100ns);

#endif
