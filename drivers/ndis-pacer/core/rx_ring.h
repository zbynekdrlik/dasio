/* dasio kernel RX (dasio#27, B23): the filter's decisions for received
 * frames of dasio's own flows, free of any kernel type.
 *
 * Every Dante audio packet goes up through tcpip, WFP and Winsock to
 * dstack's flow sockets, and a stall anywhere on that path is a late
 * packet or a hole. The filter sees each frame first, on the NIC's DPC:
 * a frame whose (destination IPv4, destination UDP port) is in a ring's
 * match table is copied into that ring, a block of memory shared with
 * the dasio process, with its receive time. The NDIS glue
 * (driver/rxring.c) parses each frame outside any lock and takes the
 * filter's RX ring lock only for a frame whose port a ring names; the
 * chain work (which NBLs go up, in what pieces) is here too, on any
 * next-linked node.
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

/* What a packet's flags say. */
/* A VLAN tag: in the frame (pacer_rx_parse) or taken off by the NIC (the
 * caller, from the NBL's 802.1Q info). Dante legs are untagged on every
 * PC (owner, 18.11.2026): a tagged frame never goes into a ring. */
#define PACER_RX_PKT_TAGGED 1u
/* The Ethernet destination is a group address (I/G bit): multicast or
 * broadcast. */
#define PACER_RX_PKT_ETH_GROUP 2u
/* The NIC found a bad IP or UDP checksum (the caller, from the NBL's
 * checksum info): never put into a ring, never kept from tcpip. */
#define PACER_RX_PKT_BAD_CSUM 4u

/* One received UDP packet's addresses, where its payload is, and its
 * flags. */
struct pacer_rx_packet {
    uint8_t src_ip[4];
    uint8_t dst_ip[4];
    uint16_t src_port;
    uint16_t dst_port;
    /* From the frame's first byte. */
    uint32_t payload_off;
    uint32_t payload_len;
    uint32_t flags;
};

/* Reads the headers of a frame of `frame_len` bytes from its first
 * `avail` bytes (`avail` <= `frame_len`; only those are read). Returns 1
 * and fills `p` for Ethernet II, IPv4 (version 4, header 20..60 bytes),
 * not a fragment (MF clear, offset 0), UDP, with the IP total length
 * inside the frame and the UDP length (at least 8) inside the IP
 * payload; the payload is the UDP length's. Up to 2 VLAN tags (0x8100 /
 * 0x88A8) are read past so a tagged frame of a flow can be counted: a tag
 * with a VLAN ID sets PACER_RX_PKT_TAGGED, a priority tag (VLAN ID 0)
 * does not (as the NIC's own 802.1Q info, where VLAN ID 0 is untagged).
 * The Ethernet group bit sets PACER_RX_PKT_ETH_GROUP. 0 for anything
 * else, or when a header lies past `avail`. */
int pacer_rx_parse(const uint8_t *hdr, uint32_t avail, uint32_t frame_len, struct pacer_rx_packet *p);

/* One match entry: a destination IPv4 address (as on the wire) and UDP
 * port. Address 0.0.0.0 matches any unicast destination on that port
 * (Ethernet unicast too, so a subnet broadcast never matches): the flows
 * sent to this PC's address, but also another host's unicast that a
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

/* Writes the header of a ring at `base` and sets up `r`. Nothing of the
 * memory's earlier contents may reach the reader: `zeroed` 1 says the
 * caller's allocator zeroed it (the driver's ExAllocatePool2), else the
 * whole ring is zeroed here first. Returns 1, or 0 when `slots` is not
 * valid (nothing written). */
int pacer_rx_ring_init(struct pacer_rx_ring *r, uint8_t *base, uint32_t slots, int zeroed);

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
    /* matched = delivered + full + too_big. */
    uint64_t matched;
    uint64_t delivered;
    uint64_t full;
    uint64_t stolen;
    uint64_t too_big;
    /* Its table named the packet, but it was tagged or its checksum bad:
     * not put, not kept from tcpip, not in matched. */
    uint64_t tagged;
    uint64_t bad_csum;
};

/* Replaces `s`'s match table with `count` entries of `table`. Returns 1,
 * or 0 when the table is not valid (pacer_rx_match_valid; nothing
 * changes). */
int pacer_rx_set_match(struct pacer_rx_sink *s, const struct pacer_rx_match_entry *table, uint32_t count);

/* 1 when any of `count` sinks matches the packet. */
int pacer_rx_wanted(struct pacer_rx_sink *const *sinks, uint32_t count, const struct pacer_rx_packet *p);

/* One received frame for `count` sinks: put into every sink whose table
 * matches (each counts it in matched, then in delivered, full or
 * too_big). A tagged packet or one with a bad checksum is only counted
 * (tagged, bad_csum) in each sink whose table names it. `frame` holds the
 * whole frame of `len` bytes, or is NULL when the caller could not read
 * it (counted as too big). Returns 1 when the frame must not go up to
 * tcpip: at least one sink matched, every matching sink steals, and none
 * counted it too big (each matching sink then counts it in stolen; a full
 * stealing ring drops it). 0: it goes up as before. */
int pacer_rx_deliver(struct pacer_rx_sink *const *sinks, uint32_t count, const struct pacer_rx_packet *p,
                     const uint8_t *frame, uint32_t len, uint64_t time_100ns);

/* The ports of every sink's table, for a look with no lock: a frame on
 * another port is never offered (the lock is not taken for it). One bit
 * per port. A rebuild (under the lock, whenever a ring or a table
 * changes) first sets the bit of every port a table names, then clears
 * the bits of the ports it recorded that no table names any more, each
 * byte written whole with only its own bits changed: a reader racing it
 * never misses a port a table named before and after, and at worst takes
 * the lock for a port just dropped. Ports past PACER_RX_MAX_PORTS distinct
 * ones are not recorded: their bits stay set (the lock taken for nothing,
 * never a missed frame). count, written once at the end of a rebuild: 0
 * when no table names any port (the receive path does nothing), else the
 * ports named (past PACER_RX_MAX_PORTS distinct ones a repeat may count
 * again). Starts all zero (pacer_rx_ports_init). */
#define PACER_RX_MAX_PORTS 64u
struct pacer_rx_ports {
    uint8_t bits[65536u / 8u];
    uint16_t recorded[PACER_RX_MAX_PORTS];
    uint32_t recorded_count;
    uint32_t count;
};

void pacer_rx_ports_init(struct pacer_rx_ports *ports);

void pacer_rx_ports_build(struct pacer_rx_sink *const *sinks, uint32_t count, struct pacer_rx_ports *ports);

/* 1 when a sink's table may name `port`. */
int pacer_rx_ports_has(const struct pacer_rx_ports *ports, uint16_t port);

/* The chain work of the receive path, on any next-linked node (an NBL in
 * the driver, a test's node on the host). */
struct pacer_rx_chain_ops {
    void *(*next)(void *node);
    void (*set_next)(void *node, void *next);
    /* Offers one node to the rings; 1 when it is kept from tcpip. */
    int (*offer)(void *ctx, void *node);
    /* Sends `count` linked nodes from `head` up (the last one's next is
     * NULL during the call). */
    void (*indicate)(void *ctx, void *head, uint32_t count);
};

/* Nodes that may pend: offers each node of `chain` once, in order, and
 * splits it into the kept nodes (returned, `*kept_count` of them) and the
 * taken ones (`*taken`, for the caller to give back), each in its
 * original order and NULL-terminated. Links are written only from the
 * first taken node on: with none taken the chain is untouched. */
void *pacer_rx_split(void *chain, const struct pacer_rx_chain_ops *ops, void *ctx, void **taken,
                     uint32_t *kept_count);

/* Nodes that cannot pend (the miniport needs its chain back as it was):
 * offers the nodes PACER_RX_SEGMENT at a time, then indicates the
 * segment's runs of consecutive kept nodes, each as a chain of its own
 * (its last link cut for the call and restored right after); taken nodes
 * are not indicated. A chain of at most PACER_RX_SEGMENT nodes with none
 * taken goes up whole and untouched. Returns the nodes indicated. */
#define PACER_RX_SEGMENT 64u
uint32_t pacer_rx_up_in_runs(void *chain, const struct pacer_rx_chain_ops *ops, void *ctx);

#endif
