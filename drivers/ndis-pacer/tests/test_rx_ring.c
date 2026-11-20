/* Unit tests of kernel RX's decisions and ring producer (dasio#27, B23;
 * no kernel, any C compiler). */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "../core/rx_ring.h"

static int failed;
static int checks;

#define CHECK(cond)                                                              \
    do {                                                                         \
        checks++;                                                                \
        if (!(cond)) {                                                           \
            failed++;                                                            \
            printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond);               \
        }                                                                        \
    } while (0)

static void put16(uint8_t *p, uint32_t v)
{
    p[0] = (uint8_t)(v >> 8);
    p[1] = (uint8_t)v;
}

static uint32_t get16le(const uint8_t *p) { return (uint32_t)p[0] | ((uint32_t)p[1] << 8); }

static uint32_t get32le(const uint8_t *p) { return get16le(p) | (get16le(p + 2) << 16); }

static uint64_t get64le(const uint8_t *p) { return (uint64_t)get32le(p) | ((uint64_t)get32le(p + 4) << 32); }

static void set64le(uint8_t *p, uint64_t v)
{
    int i;
    for (i = 0; i < 8; i++) {
        p[i] = (uint8_t)(v >> (8 * i));
    }
}

/* What a test frame carries. */
struct spec {
    int tags;          /* VLAN tags, 0..3 */
    uint32_t ihl;      /* IPv4 header bytes */
    uint16_t frag;     /* flags and fragment offset */
    uint8_t proto;
    uint8_t src[4];
    uint8_t dst[4];
    uint16_t sport;
    uint16_t dport;
    uint32_t payload;  /* UDP payload bytes */
    uint32_t pad;      /* bytes after the IP packet (Ethernet padding) */
};

static struct spec plain(void)
{
    struct spec s;
    memset(&s, 0, sizeof s);
    s.ihl = 20;
    s.proto = 17;
    s.src[0] = 10;
    s.src[1] = 77;
    s.src[2] = 7;
    s.src[3] = 30;
    s.dst[0] = 239;
    s.dst[1] = 69;
    s.dst[2] = 1;
    s.dst[3] = 2;
    s.sport = 4321;
    s.dport = 4321;
    s.payload = 100;
    return s;
}

/* Writes the frame into `f` (big enough) and returns its length; `*ip_at`
 * gets where the IPv4 header starts. Payload byte i is (i * 7 + 1). */
static uint32_t frame(uint8_t *f, const struct spec *s, uint32_t *ip_at)
{
    uint32_t at = 12;
    uint32_t total = s->ihl + 8u + s->payload;
    uint32_t i;
    int t;
    memset(f, 0, 2400);
    for (i = 0; i < 12; i++) {
        f[i] = (uint8_t)(0xA0 + i);
    }
    for (t = 0; t < s->tags; t++) {
        put16(f + at, t == 0 && s->tags >= 2 ? 0x88A8u : 0x8100u);
        put16(f + at + 2, 100u + (uint32_t)t);
        at += 4;
    }
    put16(f + at, 0x0800u);
    at += 2;
    *ip_at = at;
    f[at] = (uint8_t)(0x40 | (s->ihl / 4u));
    f[at + 1] = 46u << 2;
    put16(f + at + 2, total);
    put16(f + at + 6, s->frag);
    f[at + 8] = 64;
    f[at + 9] = s->proto;
    memcpy(f + at + 12, s->src, 4);
    memcpy(f + at + 16, s->dst, 4);
    put16(f + at + s->ihl, s->sport);
    put16(f + at + s->ihl + 2, s->dport);
    put16(f + at + s->ihl + 4, 8u + s->payload);
    for (i = 0; i < s->payload; i++) {
        f[at + s->ihl + 8 + i] = (uint8_t)(i * 7u + 1u);
    }
    return at + total + s->pad;
}

static void parse(void)
{
    uint8_t f[2400];
    struct spec s;
    struct pacer_rx_packet p;
    uint32_t n;
    uint32_t ip;

    /* A Dante audio packet: addresses, ports and payload. */
    s = plain();
    n = frame(f, &s, &ip);
    CHECK(n == 14 + 20 + 8 + 100);
    memset(&p, 0xEE, sizeof p);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1);
    CHECK(memcmp(p.src_ip, s.src, 4) == 0 && memcmp(p.dst_ip, s.dst, 4) == 0);
    CHECK(p.src_port == 4321 && p.dst_port == 4321);
    CHECK(p.payload_off == 42 && p.payload_len == 100);
    CHECK(f[p.payload_off] == 1 && f[p.payload_off + 99] == (99 * 7 + 1) % 256);
    CHECK(p.flags == 0);
    /* The Ethernet group bit (a multicast or broadcast MAC). */
    f[0] = 0x01;
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.flags == PACER_RX_PKT_ETH_GROUP);
    f[0] = 0xFE;
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.flags == 0);

    /* Distinct ports land in their own fields. */
    s.sport = 1234;
    s.dport = 5004;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.src_port == 1234 && p.dst_port == 5004);

    /* One and two VLAN tags shift everything; a third is not skipped. */
    s = plain();
    s.tags = 1;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.payload_off == 46 && p.payload_len == 100);
    CHECK(p.flags == PACER_RX_PKT_TAGGED);
    s.tags = 2;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.payload_off == 50 && p.flags == PACER_RX_PKT_TAGGED);
    /* A priority tag (VLAN ID 0) is untagged traffic, as the NIC's own
     * 802.1Q info has it; any VLAN ID bit counts, the priority bits not. */
    s.tags = 1;
    n = frame(f, &s, &ip);
    f[14] = 0xE0; /* priority 7, VLAN ID 0 */
    f[15] = 0x00;
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.payload_off == 46 && p.flags == 0);
    f[14] = 0x08; /* VLAN ID 0x800 */
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.flags == PACER_RX_PKT_TAGGED);
    f[14] = 0xE0;
    f[15] = 0x01; /* VLAN ID 1 */
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.flags == PACER_RX_PKT_TAGGED);
    /* Two tags: the inner one's VLAN ID counts too. */
    s.tags = 2;
    n = frame(f, &s, &ip);
    f[14] = 0;
    f[15] = 0;
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.flags == PACER_RX_PKT_TAGGED);
    f[18] = 0;
    f[19] = 0;
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.flags == 0);
    s.tags = 3;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    /* A tag with nothing readable after it. */
    s.tags = 1;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, 17, n, &p) == 0);
    CHECK(pacer_rx_parse(f, 18, n, &p) == 0);

    /* IPv4 options: the UDP header follows them. */
    s = plain();
    s.ihl = 24;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.payload_off == 14 + 24 + 8 && p.dst_port == 4321);
    s.ihl = 60;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.payload_off == 14 + 60 + 8);

    /* Ethernet padding after a short packet is not payload. */
    s = plain();
    s.payload = 4;
    s.pad = 14;
    n = frame(f, &s, &ip);
    CHECK(n == 60);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.payload_len == 4);

    /* An empty datagram. */
    s.payload = 0;
    s.pad = 0;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.payload_len == 0 && p.payload_off == n);

    /* Don't-fragment is fine; MF or an offset is a fragment. */
    s = plain();
    s.frag = 0x4000;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1);
    s.frag = 0x2000;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    s.frag = 0x0001;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    s.frag = 0x1000;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    s.frag = 0x8000; /* the reserved bit alone */
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1);

    /* Not UDP, not IPv4. */
    s = plain();
    s.proto = 6;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    s = plain();
    n = frame(f, &s, &ip);
    put16(f + 12, 0x86DDu);
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    put16(f + 12, 0x0806u);
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    put16(f + 12, 0x0800u);
    f[ip] = 0x65; /* version 6 in an IPv4 frame */
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    f[ip] = 0x44; /* a 16-byte header */
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    f[ip] = 0x45;
    CHECK(pacer_rx_parse(f, n, n, &p) == 1);

    /* Nothing, or fewer readable bytes than the headers need. */
    CHECK(pacer_rx_parse(NULL, n, n, &p) == 0);
    CHECK(pacer_rx_parse(f, 13, n, &p) == 0);
    CHECK(pacer_rx_parse(f, 14 + 19, n, &p) == 0);
    CHECK(pacer_rx_parse(f, 14 + 20 + 7, n, &p) == 0);
    CHECK(pacer_rx_parse(f, 14 + 20 + 8, n, &p) == 1 && p.payload_len == 100);
    /* More readable than the frame holds: refused. */
    CHECK(pacer_rx_parse(f, n + 1, n, &p) == 0);
    /* The headers alone, the frame longer (the driver reads 128 bytes). */
    CHECK(pacer_rx_parse(f, 128, n, &p) == 1 && p.payload_len == 100);

    /* The IP total length: at least header + UDP header, inside the frame. */
    s = plain();
    n = frame(f, &s, &ip);
    put16(f + ip + 2, 27);
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    put16(f + ip + 2, 28);
    put16(f + ip + 20 + 4, 8);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.payload_len == 0);
    put16(f + ip + 2, n - ip + 1);
    put16(f + ip + 20 + 4, 108);
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    /* At the end of the frame exactly, and the frame cut short. */
    put16(f + ip + 2, n - ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1);
    CHECK(pacer_rx_parse(f, n - 1, n - 1, &p) == 0);
    /* An option-laden header: the total is checked after the tags too. */
    s = plain();
    s.tags = 2;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1);
    CHECK(pacer_rx_parse(f, n - 1, n - 1, &p) == 0);

    /* The UDP length: at least 8, inside the IP payload; a shorter one
     * than the IP payload is the datagram (the rest is trailer). */
    s = plain();
    n = frame(f, &s, &ip);
    put16(f + ip + 20 + 4, 7);
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    put16(f + ip + 20 + 4, 8 + 101);
    CHECK(pacer_rx_parse(f, n, n, &p) == 0);
    put16(f + ip + 20 + 4, 8 + 100);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.payload_len == 100);
    put16(f + ip + 20 + 4, 8 + 60);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1 && p.payload_len == 60);
}

static struct pacer_rx_packet to(uint8_t a, uint8_t b, uint8_t c, uint8_t d, uint16_t port)
{
    struct pacer_rx_packet p;
    memset(&p, 0, sizeof p);
    p.dst_ip[0] = a;
    p.dst_ip[1] = b;
    p.dst_ip[2] = c;
    p.dst_ip[3] = d;
    p.dst_port = port;
    return p;
}

static struct pacer_rx_match_entry entry(uint8_t a, uint8_t b, uint8_t c, uint8_t d, uint16_t port)
{
    struct pacer_rx_match_entry e;
    e.addr[0] = a;
    e.addr[1] = b;
    e.addr[2] = c;
    e.addr[3] = d;
    e.port = port;
    return e;
}

static void match(void)
{
    struct pacer_rx_match_entry t[17];
    struct pacer_rx_packet p;
    int i;

    /* A multicast group matches itself only, on its port (its frames go
     * to a group MAC). */
    t[0] = entry(239, 69, 1, 2, 4321);
    p = to(239, 69, 1, 2, 4321);
    CHECK(pacer_rx_match(t, 1, &p) == 1);
    p.flags = PACER_RX_PKT_ETH_GROUP;
    CHECK(pacer_rx_match(t, 1, &p) == 1);
    p = to(239, 69, 1, 3, 4321);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    p = to(239, 69, 1, 2, 4322);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    /* Every address byte counts. */
    p = to(238, 69, 1, 2, 4321);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    p = to(239, 68, 1, 2, 4321);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    p = to(239, 69, 0, 2, 4321);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    /* An empty table matches nothing. */
    p = to(239, 69, 1, 2, 4321);
    CHECK(pacer_rx_match(t, 0, &p) == 0);

    /* 0.0.0.0: any unicast address on that port, never a group or the
     * broadcast. */
    t[0] = entry(0, 0, 0, 0, 14336);
    p = to(10, 77, 7, 30, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 1);
    p = to(10, 77, 7, 30, 14337);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    p = to(224, 0, 0, 0, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    p = to(239, 255, 255, 255, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    p = to(223, 255, 255, 255, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 1);
    p = to(240, 0, 0, 1, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 1);
    p = to(255, 255, 255, 255, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    p = to(0, 255, 255, 255, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 1);
    p = to(255, 0, 255, 255, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 1);
    p = to(255, 255, 0, 255, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 1);
    p = to(255, 255, 255, 0, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 1);
    /* A unicast address in a frame to a group MAC (a subnet broadcast:
     * 10.77.7.255 to ff:ff:ff:ff:ff:ff) is not "any". */
    p = to(10, 77, 7, 255, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 1);
    p.flags = PACER_RX_PKT_ETH_GROUP;
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    /* Destination 0.0.0.0 itself: an "any" entry names it only in a frame
     * to a unicast MAC, as any other unicast address. */
    p = to(0, 0, 0, 0, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 1);
    p.flags = PACER_RX_PKT_ETH_GROUP;
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    p = to(10, 77, 7, 255, 14336);
    /* Other flags do not change the match (deliver judges them). */
    p.flags = PACER_RX_PKT_TAGGED | PACER_RX_PKT_BAD_CSUM;
    CHECK(pacer_rx_match(t, 1, &p) == 1);
    /* An address with any non-zero byte is not "any". */
    p = to(10, 77, 7, 30, 14336);
    t[0] = entry(1, 0, 0, 0, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    t[0] = entry(0, 1, 0, 0, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    t[0] = entry(0, 0, 1, 0, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    t[0] = entry(0, 0, 0, 1, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 0);
    /* The broadcast named itself matches. */
    t[0] = entry(255, 255, 255, 255, 14336);
    p = to(255, 255, 255, 255, 14336);
    CHECK(pacer_rx_match(t, 1, &p) == 1);

    /* Any entry of the table; at most 16 are looked at. */
    for (i = 0; i < 17; i++) {
        t[i] = entry(239, 69, 1, (uint8_t)i, 4321);
    }
    p = to(239, 69, 1, 15, 4321);
    CHECK(pacer_rx_match(t, 16, &p) == 1);
    CHECK(pacer_rx_match(t, 15, &p) == 0);
    p = to(239, 69, 1, 16, 4321);
    CHECK(pacer_rx_match(t, 17, &p) == 0);

    /* A table is valid with at most 16 entries, none on port 0. */
    CHECK(pacer_rx_match_valid(t, 0) == 1);
    CHECK(pacer_rx_match_valid(t, 16) == 1);
    CHECK(pacer_rx_match_valid(t, 17) == 0);
    t[3].port = 0;
    CHECK(pacer_rx_match_valid(t, 3) == 1);
    CHECK(pacer_rx_match_valid(t, 4) == 0);
}

#define SLOTS 64u

/* A ring buffer of exactly its size (AddressSanitizer stops a write past
 * it), first filled with 0xAA. */
static uint8_t *new_ring(uint32_t slots)
{
    uint32_t bytes = pacer_rx_ring_bytes(slots);
    uint8_t *b = malloc(bytes);
    if (b == NULL) {
        abort();
    }
    memset(b, 0xAA, bytes);
    return b;
}

static uint8_t *slot_at(uint8_t *base, uint32_t i)
{
    return base + PACER_RX_HEADER_BYTES + i * PACER_RX_SLOT_BYTES;
}

static void ring(void)
{
    uint8_t f[2400];
    struct spec s;
    struct pacer_rx_packet p;
    struct pacer_rx_ring r;
    uint8_t *b;
    uint8_t *slot;
    uint32_t n;
    uint32_t ip;
    uint32_t i;
    int wake;
    int zero = 1;
    uint8_t untouched[16];

    /* Sizes: a power of 2 slots in 64..4096. */
    CHECK(pacer_rx_ring_bytes(64) == 4096u + 64u * 2048u);
    CHECK(pacer_rx_ring_bytes(4096) == 4096u + 4096u * 2048u);
    CHECK(pacer_rx_ring_bytes(128) == 4096u + 128u * 2048u);
    CHECK(pacer_rx_ring_bytes(0) == 0);
    CHECK(pacer_rx_ring_bytes(32) == 0);
    CHECK(pacer_rx_ring_bytes(63) == 0);
    CHECK(pacer_rx_ring_bytes(65) == 0);
    CHECK(pacer_rx_ring_bytes(96) == 0);
    CHECK(pacer_rx_ring_bytes(8192) == 0);
    CHECK(PACER_RX_MAX_FRAME == 2016u);

    /* An invalid size writes nothing. */
    memset(untouched, 0x55, sizeof untouched);
    CHECK(pacer_rx_ring_init(&r, untouched, 100, 0) == 0);
    CHECK(pacer_rx_ring_init(&r, untouched, 100, 1) == 0);
    for (i = 0; i < sizeof untouched; i++) {
        CHECK(untouched[i] == 0x55);
    }

    /* A new ring: header, everything else zero. */
    b = new_ring(SLOTS);
    /* Memory the caller's allocator zeroed: only the header is written
     * (the driver's 8 MB are not zeroed twice). */
    CHECK(pacer_rx_ring_init(&r, b, SLOTS, 1) == 1);
    CHECK(memcmp(b, "DRXR", 4) == 0 && get32le(b + PACER_RX_OFF_DATA) == 4096);
    CHECK(b[20] == 0xAA && b[PACER_RX_OFF_HEAD] == 0xAA && b[4096] == 0xAA);
    CHECK(b[pacer_rx_ring_bytes(SLOTS) - 1] == 0xAA);
    /* Memory nobody zeroed: everything but the header is zeroed here. */
    CHECK(pacer_rx_ring_init(&r, b, SLOTS, 0) == 1);
    CHECK(memcmp(b, "DRXR", 4) == 0);
    CHECK(get32le(b + PACER_RX_OFF_VERSION) == 1);
    CHECK(get32le(b + PACER_RX_OFF_SLOTS) == SLOTS);
    CHECK(get32le(b + PACER_RX_OFF_SLOT_BYTES) == 2048);
    CHECK(get32le(b + PACER_RX_OFF_DATA) == 4096);
    CHECK(r.base == b && r.slots == SLOTS && r.head == 0 && r.tail_seen == 0);
    for (i = 20; i < pacer_rx_ring_bytes(SLOTS); i++) {
        if (b[i] != 0) {
            zero = 0;
        }
    }
    CHECK(zero);

    /* One frame: metadata, the whole frame, head published. */
    s = plain();
    s.sport = 1111;
    s.dport = 4321;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1);
    wake = 0;
    CHECK(pacer_rx_put(&r, f, n, &p, 0x0123456789ABCDEFull, &wake) == PACER_RX_DELIVERED);
    CHECK(wake == 0);
    CHECK(r.head == 1 && get64le(b + PACER_RX_OFF_HEAD) == 1);
    slot = slot_at(b, 0);
    CHECK(get64le(slot) == 0x0123456789ABCDEFull);
    CHECK(memcmp(slot + 8, s.src, 4) == 0 && memcmp(slot + 12, s.dst, 4) == 0);
    CHECK(get16le(slot + 16) == 1111 && get16le(slot + 18) == 4321);
    CHECK(get16le(slot + 20) == 42 && get16le(slot + 22) == 100);
    CHECK(get16le(slot + 24) == n && get16le(slot + 26) == 0);
    CHECK(get32le(slot + 28) == 0);
    CHECK(memcmp(slot + 32, f, n) == 0);
    CHECK(slot[32 + n] == 0);

    /* The next frame goes into the next slot with the next number. */
    CHECK(pacer_rx_put(&r, f, n, &p, 7, &wake) == PACER_RX_DELIVERED);
    CHECK(get32le(slot_at(b, 1) + 28) == 1 && get64le(slot_at(b, 1)) == 7);
    CHECK(get64le(b + PACER_RX_OFF_HEAD) == 2);

    /* Fill it: 64 frames, nothing consumed; the 65th does not fit. */
    for (i = 2; i < SLOTS; i++) {
        CHECK(pacer_rx_put(&r, f, n, &p, i, &wake) == PACER_RX_DELIVERED);
    }
    CHECK(r.head == SLOTS);
    CHECK(pacer_rx_put(&r, f, n, &p, 99, &wake) == PACER_RX_FULL);
    CHECK(r.head == SLOTS && get64le(b + PACER_RX_OFF_HEAD) == SLOTS);
    CHECK(get64le(slot_at(b, 0)) == 0x0123456789ABCDEFull);
    /* The reader consumed one: frame 64 goes into slot 0. */
    set64le(b + PACER_RX_OFF_TAIL, 1);
    CHECK(pacer_rx_put(&r, f, n, &p, 99, &wake) == PACER_RX_DELIVERED);
    CHECK(get64le(slot_at(b, 0)) == 99 && get32le(slot_at(b, 0) + 28) == SLOTS);
    CHECK(r.tail_seen == 1);
    CHECK(pacer_rx_put(&r, f, n, &p, 100, &wake) == PACER_RX_FULL);

    /* A tail it cannot believe is a full ring: ahead of head, far behind
     * it, or anything at all. Nothing is written (ASan: never outside). */
    set64le(b + PACER_RX_OFF_TAIL, r.head + 1);
    CHECK(pacer_rx_put(&r, f, n, &p, 101, &wake) == PACER_RX_FULL);
    set64le(b + PACER_RX_OFF_TAIL, 0xFFFFFFFFFFFFFFFFull);
    CHECK(pacer_rx_put(&r, f, n, &p, 101, &wake) == PACER_RX_FULL);
    set64le(b + PACER_RX_OFF_TAIL, 0);
    CHECK(pacer_rx_put(&r, f, n, &p, 101, &wake) == PACER_RX_FULL);
    CHECK(get64le(slot_at(b, 1)) == 7);
    /* Everything consumed: room again. */
    set64le(b + PACER_RX_OFF_TAIL, r.head);
    CHECK(pacer_rx_put(&r, f, n, &p, 102, &wake) == PACER_RX_DELIVERED);
    CHECK(get64le(slot_at(b, 1)) == 102);
    /* What the reader writes into the header does not move the writer. */
    set64le(b + PACER_RX_OFF_HEAD, 5);
    b[PACER_RX_OFF_SLOTS] = 0xFF;
    CHECK(pacer_rx_put(&r, f, n, &p, 103, &wake) == PACER_RX_DELIVERED);
    CHECK(get64le(b + PACER_RX_OFF_HEAD) == SLOTS + 3);
    CHECK(get64le(slot_at(b, 2)) == 103);

    /* Frames longer than a slot, or unreadable, are not put. */
    s = plain();
    s.payload = PACER_RX_MAX_FRAME - 42;
    n = frame(f, &s, &ip);
    CHECK(n == PACER_RX_MAX_FRAME);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1);
    CHECK(pacer_rx_put(&r, f, n, &p, 104, &wake) == PACER_RX_DELIVERED);
    CHECK(get16le(slot_at(b, 3) + 24) == PACER_RX_MAX_FRAME);
    CHECK(slot_at(b, 3)[32 + n - 1] == f[n - 1]);
    s.payload++;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1);
    CHECK(pacer_rx_put(&r, f, n, &p, 105, &wake) == PACER_RX_TOO_BIG);
    CHECK(pacer_rx_put(&r, NULL, 60, &p, 105, &wake) == PACER_RX_TOO_BIG);
    CHECK(get64le(b + PACER_RX_OFF_HEAD) == SLOTS + 4);

    /* Wake: set by the reader, cleared by the next frame, reported once. */
    s = plain();
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1);
    b[PACER_RX_OFF_WAKE] = 1;
    wake = 0;
    CHECK(pacer_rx_put(&r, f, n, &p, 106, &wake) == PACER_RX_DELIVERED);
    CHECK(wake == 1 && get32le(b + PACER_RX_OFF_WAKE) == 0);
    wake = 0;
    CHECK(pacer_rx_put(&r, f, n, &p, 107, &wake) == PACER_RX_DELIVERED);
    CHECK(wake == 0);
    /* A full ring wakes nobody (nothing new to read). */
    b[PACER_RX_OFF_WAKE + 3] = 1;
    set64le(b + PACER_RX_OFF_TAIL, 0);
    r.tail_seen = 0;
    CHECK(pacer_rx_put(&r, f, n, &p, 108, &wake) == PACER_RX_FULL);
    CHECK(wake == 0 && get32le(b + PACER_RX_OFF_WAKE) == 0x01000000u);

    /* Detached: the reader sees the word, nothing else moves. */
    CHECK(get32le(b + PACER_RX_OFF_DETACHED) == 0);
    pacer_rx_ring_detach(&r);
    CHECK(get32le(b + PACER_RX_OFF_DETACHED) == 1);
    CHECK(get64le(b + PACER_RX_OFF_HEAD) == SLOTS + 6 && r.head == SLOTS + 6);
    CHECK(get32le(b + PACER_RX_OFF_DETACHED + 4) == 0 && get32le(b + PACER_RX_OFF_DETACHED - 4) == 0);
    free(b);

    /* The largest ring: the last slot's last byte is its memory's last. */
    b = new_ring(PACER_RX_MAX_SLOTS);
    CHECK(pacer_rx_ring_init(&r, b, PACER_RX_MAX_SLOTS, 0) == 1);
    s = plain();
    s.payload = PACER_RX_MAX_FRAME - 42;
    n = frame(f, &s, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1);
    r.head = PACER_RX_MAX_SLOTS - 1;
    r.tail_seen = r.head;
    CHECK(pacer_rx_put(&r, f, n, &p, 1, &wake) == PACER_RX_DELIVERED);
    CHECK(b[pacer_rx_ring_bytes(PACER_RX_MAX_SLOTS) - 1] == f[n - 1]);
    free(b);
}

static void sink_init(struct pacer_rx_sink *s, uint8_t *b, uint32_t steal)
{
    memset(s, 0, sizeof *s);
    CHECK(pacer_rx_ring_init(&s->ring, b, SLOTS, 0) == 1);
    s->steal = steal;
}

static void sinks(void)
{
    uint8_t f[2400];
    struct spec sp;
    struct pacer_rx_packet p;
    struct pacer_rx_packet other;
    struct pacer_rx_sink a;
    struct pacer_rx_sink c;
    struct pacer_rx_sink *list[2];
    struct pacer_rx_match_entry t[17];
    uint8_t *ba = new_ring(SLOTS);
    uint8_t *bc = new_ring(SLOTS);
    uint32_t n;
    uint32_t ip;
    uint32_t i;

    sp = plain();
    n = frame(f, &sp, &ip);
    CHECK(pacer_rx_parse(f, n, n, &p) == 1);
    other = p;
    other.dst_port = 4322;

    /* The table is replaced whole; an invalid one changes nothing. */
    sink_init(&a, ba, 0);
    t[0] = entry(239, 69, 1, 2, 4321);
    t[1] = entry(0, 0, 0, 0, 14336);
    CHECK(pacer_rx_set_match(&a, t, 2) == 1);
    CHECK(a.matches == 2 && a.match[1].port == 14336);
    t[1].port = 0;
    CHECK(pacer_rx_set_match(&a, t, 2) == 0);
    CHECK(a.matches == 2 && a.match[1].port == 14336);
    for (i = 0; i < 17; i++) {
        t[i] = entry(239, 1, 1, (uint8_t)i, 5004);
    }
    CHECK(pacer_rx_set_match(&a, t, 17) == 0);
    CHECK(a.matches == 2);
    t[0] = entry(239, 69, 1, 2, 4321);
    CHECK(pacer_rx_set_match(&a, t, 1) == 1 && a.matches == 1);
    CHECK(pacer_rx_set_match(&a, t, 0) == 1 && a.matches == 0);
    CHECK(pacer_rx_set_match(&a, t, 1) == 1);

    /* Copy mode: the frame goes into the ring and up. */
    list[0] = &a;
    CHECK(pacer_rx_wanted(list, 1, &p) == 1);
    CHECK(pacer_rx_wanted(list, 1, &other) == 0);
    CHECK(pacer_rx_wanted(list, 0, &p) == 0);
    CHECK(pacer_rx_deliver(list, 1, &p, f, n, 5) == 0);
    CHECK(a.matched == 1 && a.delivered == 1 && a.stolen == 0 && a.full == 0 && a.too_big == 0);
    CHECK(get64le(ba + PACER_RX_OFF_HEAD) == 1);
    /* Not its flow: nothing counted, it goes up. */
    CHECK(pacer_rx_deliver(list, 1, &other, f, n, 5) == 0);
    CHECK(a.matched == 1 && a.delivered == 1);

    /* Steal mode: into the ring, not up. */
    a.steal = 1;
    CHECK(pacer_rx_deliver(list, 1, &p, f, n, 6) == 1);
    CHECK(a.matched == 2 && a.delivered == 2 && a.stolen == 1);
    CHECK(pacer_rx_deliver(list, 1, &other, f, n, 6) == 0);
    CHECK(a.stolen == 1);
    /* Too big for a slot: up, even in steal mode. */
    CHECK(pacer_rx_deliver(list, 1, &p, NULL, n, 7) == 0);
    CHECK(pacer_rx_deliver(list, 1, &p, f, PACER_RX_MAX_FRAME + 1, 7) == 0);
    CHECK(a.matched == 4 && a.too_big == 2 && a.stolen == 1);
    /* The reader asked for a wake. */
    ba[PACER_RX_OFF_WAKE] = 1;
    CHECK(pacer_rx_deliver(list, 1, &p, f, n, 8) == 1);
    CHECK(a.wake == 1);
    a.wake = 0;

    /* Two rings on one group: each gets the frame. Both steal: not up;
     * one copies: up, and nobody counts it stolen. */
    sink_init(&c, bc, 1);
    CHECK(pacer_rx_set_match(&c, t, 1) == 1);
    list[1] = &c;
    CHECK(pacer_rx_deliver(list, 2, &p, f, n, 9) == 1);
    CHECK(a.delivered == 4 && c.delivered == 1 && a.stolen == 3 && c.stolen == 1);
    c.steal = 0;
    CHECK(pacer_rx_deliver(list, 2, &p, f, n, 10) == 0);
    CHECK(a.delivered == 5 && c.delivered == 2 && a.stolen == 3 && c.stolen == 1);
    c.steal = 1;
    /* A stealing ring that does not match leaves the decision to the
     * matching ones. */
    t[0] = entry(239, 69, 1, 9, 4321);
    CHECK(pacer_rx_set_match(&c, t, 1) == 1);
    a.steal = 0;
    CHECK(pacer_rx_wanted(list, 2, &p) == 1);
    CHECK(pacer_rx_deliver(list, 2, &p, f, n, 11) == 0);
    CHECK(c.matched == 2 && c.stolen == 1);
    a.steal = 1;
    CHECK(pacer_rx_deliver(list, 2, &p, f, n, 12) == 1);
    CHECK(a.stolen == 4 && c.stolen == 1 && c.matched == 2);
    /* The second ring alone matches the other packet. */
    other = p;
    other.dst_ip[3] = 9;
    CHECK(pacer_rx_wanted(list, 2, &other) == 1);
    CHECK(pacer_rx_deliver(list, 2, &other, f, n, 13) == 1);
    CHECK(c.matched == 3 && c.stolen == 2 && a.stolen == 4);

    /* A full stealing ring drops the frame and counts it. */
    a.ring.head = a.ring.tail_seen + SLOTS;
    set64le(ba + PACER_RX_OFF_TAIL, a.ring.tail_seen);
    CHECK(pacer_rx_deliver(list, 1, &p, f, n, 14) == 1);
    CHECK(a.full == 1 && a.stolen == 5);
    /* A full copying ring: counted, and up. */
    a.steal = 0;
    CHECK(pacer_rx_deliver(list, 1, &p, f, n, 15) == 0);
    CHECK(a.full == 2 && a.stolen == 5);

    /* Tagged or a bad checksum: counted where the table names it, never
     * put, never kept from tcpip, not in matched. */
    sink_init(&a, ba, 1);
    sink_init(&c, bc, 1);
    t[0] = entry(239, 69, 1, 2, 4321);
    CHECK(pacer_rx_set_match(&a, t, 1) == 1 && pacer_rx_set_match(&c, t, 1) == 1);
    list[0] = &a;
    list[1] = &c;
    other = p;
    other.flags = PACER_RX_PKT_TAGGED;
    CHECK(pacer_rx_deliver(list, 2, &other, f, n, 16) == 0);
    CHECK(a.tagged == 1 && c.tagged == 1 && a.matched == 0 && a.delivered == 0 && a.stolen == 0);
    CHECK(get64le(ba + PACER_RX_OFF_HEAD) == 0);
    other.flags = PACER_RX_PKT_BAD_CSUM;
    CHECK(pacer_rx_deliver(list, 2, &other, f, n, 17) == 0);
    CHECK(a.bad_csum == 1 && c.bad_csum == 1 && a.tagged == 1 && a.matched == 0 && c.stolen == 0);
    /* Both: tagged wins. */
    other.flags = PACER_RX_PKT_TAGGED | PACER_RX_PKT_BAD_CSUM;
    CHECK(pacer_rx_deliver(list, 2, &other, f, n, 18) == 0);
    CHECK(a.tagged == 2 && a.bad_csum == 1);
    /* Not its flow: nothing counted. */
    other.dst_port = 9;
    CHECK(pacer_rx_deliver(list, 2, &other, f, n, 19) == 0);
    CHECK(a.tagged == 2 && a.bad_csum == 1);
    /* The group bit alone changes nothing for a group entry. */
    other = p;
    other.flags = PACER_RX_PKT_ETH_GROUP;
    CHECK(pacer_rx_deliver(list, 2, &other, f, n, 20) == 1);
    CHECK(a.delivered == 1 && c.delivered == 1 && a.stolen == 1 && c.stolen == 1);

    free(ba);
    free(bc);
}

/* Writes into `t` 16 entries on ports first..first+15. */
static void sixteen(struct pacer_rx_match_entry *t, uint32_t first)
{
    uint32_t i;
    for (i = 0; i < 16; i++) {
        t[i] = entry(239, 1, 1, 1, (uint16_t)(first + i));
    }
}

static void ports(void)
{
    static struct pacer_rx_ports pt;
    struct pacer_rx_sink a;
    struct pacer_rx_sink c;
    struct pacer_rx_sink many[5];
    struct pacer_rx_sink *list[5];
    struct pacer_rx_match_entry t[16];
    uint32_t k;
    uint32_t port;
    int all;

    memset(&a, 0, sizeof a);
    memset(&c, 0, sizeof c);
    list[0] = &a;
    list[1] = &c;
    /* Empty, then no ring, or no entries: no port. */
    memset(&pt, 0xFF, sizeof pt);
    pacer_rx_ports_init(&pt);
    CHECK(pt.count == 0 && pt.recorded_count == 0 && pacer_rx_ports_has(&pt, 4321) == 0);
    pacer_rx_ports_build(list, 0, &pt);
    CHECK(pt.count == 0 && pacer_rx_ports_has(&pt, 4321) == 0);
    pacer_rx_ports_build(list, 2, &pt);
    CHECK(pt.count == 0 && pacer_rx_ports_has(&pt, 4321) == 0);
    /* Every table's ports, each once. */
    t[0] = entry(239, 69, 1, 2, 4321);
    t[1] = entry(0, 0, 0, 0, 14336);
    t[2] = entry(239, 69, 1, 3, 4321);
    CHECK(pacer_rx_set_match(&a, t, 3) == 1);
    t[0] = entry(239, 69, 1, 9, 5004);
    t[1] = entry(239, 69, 1, 2, 14336);
    CHECK(pacer_rx_set_match(&c, t, 2) == 1);
    pacer_rx_ports_build(list, 2, &pt);
    CHECK(pt.count == 3 && pt.recorded_count == 3);
    CHECK(pacer_rx_ports_has(&pt, 4321) && pacer_rx_ports_has(&pt, 14336) && pacer_rx_ports_has(&pt, 5004));
    CHECK(pacer_rx_ports_has(&pt, 4322) == 0 && pacer_rx_ports_has(&pt, 4320) == 0);
    CHECK(pacer_rx_ports_has(&pt, 0) == 0 && pacer_rx_ports_has(&pt, 65535) == 0);
    /* A ring gone: its own ports go, a port another ring names stays. */
    pacer_rx_ports_build(list, 1, &pt);
    CHECK(pt.count == 2 && pacer_rx_ports_has(&pt, 5004) == 0);
    CHECK(pacer_rx_ports_has(&pt, 4321) && pacer_rx_ports_has(&pt, 14336));
    /* A table changed in place ([4321, 14336] to [7, 4321, 14336]): the
     * ports both name keep their bits, the new one is added. */
    t[0] = entry(239, 69, 1, 7, 7);
    t[1] = entry(239, 69, 1, 2, 4321);
    t[2] = entry(0, 0, 0, 0, 14336);
    CHECK(pacer_rx_set_match(&a, t, 3) == 1);
    pacer_rx_ports_build(list, 1, &pt);
    CHECK(pt.count == 3 && pacer_rx_ports_has(&pt, 7) && pacer_rx_ports_has(&pt, 4321) &&
          pacer_rx_ports_has(&pt, 14336));
    /* The extremes of the port range. */
    t[0] = entry(239, 69, 1, 7, 1);
    t[1] = entry(239, 69, 1, 7, 65535);
    CHECK(pacer_rx_set_match(&a, t, 2) == 1);
    pacer_rx_ports_build(list, 1, &pt);
    CHECK(pt.count == 2 && pacer_rx_ports_has(&pt, 1) && pacer_rx_ports_has(&pt, 65535));
    CHECK(pacer_rx_ports_has(&pt, 7) == 0 && pacer_rx_ports_has(&pt, 4321) == 0 && pacer_rx_ports_has(&pt, 0) == 0);
    /* The other ring back: its ports again. Then every table empty:
     * nothing. */
    pacer_rx_ports_build(list, 2, &pt);
    CHECK(pt.count == 4 && pacer_rx_ports_has(&pt, 5004) && pacer_rx_ports_has(&pt, 14336));
    CHECK(pacer_rx_set_match(&a, t, 0) == 1 && pacer_rx_set_match(&c, t, 0) == 1);
    pacer_rx_ports_build(list, 2, &pt);
    CHECK(pt.count == 0 && pacer_rx_ports_has(&pt, 1) == 0 && pacer_rx_ports_has(&pt, 65535) == 0);
    CHECK(pacer_rx_ports_has(&pt, 5004) == 0 && pacer_rx_ports_has(&pt, 14336) == 0);

    /* 4 rings of 16 distinct ports are all recorded and all go again. */
    for (k = 0; k < 5; k++) {
        memset(&many[k], 0, sizeof many[k]);
        sixteen(t, 1000 + k * 16);
        CHECK(pacer_rx_set_match(&many[k], t, 16) == 1);
        list[k] = &many[k];
    }
    pacer_rx_ports_init(&pt);
    pacer_rx_ports_build(list, 4, &pt);
    CHECK(pt.count == 64 && pt.recorded_count == 64);
    CHECK(pacer_rx_ports_has(&pt, 1000) && pacer_rx_ports_has(&pt, 1063) && !pacer_rx_ports_has(&pt, 1064));
    pacer_rx_ports_build(list, 0, &pt);
    all = 0;
    for (port = 1000; port < 1064; port++) {
        all += pacer_rx_ports_has(&pt, (uint16_t)port);
    }
    CHECK(pt.count == 0 && all == 0);
    /* A fifth ring's ports past the 64 recorded: set (never missed), and
     * kept set when dropped (the lock taken for nothing, never a miss). */
    pacer_rx_ports_build(list, 5, &pt);
    CHECK(pt.count == 80 && pt.recorded_count == 64);
    CHECK(pacer_rx_ports_has(&pt, 1064) && pacer_rx_ports_has(&pt, 1079));
    pacer_rx_ports_build(list, 0, &pt);
    CHECK(pt.count == 0 && pacer_rx_ports_has(&pt, 1063) == 0 && pacer_rx_ports_has(&pt, 1079) == 1);
}

/* A test node for the chain work: its index and a next link. */
struct node {
    struct node *next;
    int index;
};

struct chain_test {
    const char *take; /* '1' at an index: offered and taken */
    int offered[200];
    int offers;
    /* What indicate saw: each call's first index and count, and every
     * index in order. */
    int call_first[200];
    uint32_t call_count[200];
    int calls;
    int up[200];
    int ups;
    int bad_run;
};

static void *node_next(void *n) { return ((struct node *)n)->next; }

static void node_set_next(void *n, void *next) { ((struct node *)n)->next = next; }

static int node_offer(void *ctx, void *n)
{
    struct chain_test *t = ctx;
    int i = ((struct node *)n)->index;
    t->offered[t->offers++] = i;
    return t->take[i] == '1';
}

static void node_indicate(void *ctx, void *head, uint32_t count)
{
    struct chain_test *t = ctx;
    struct node *n = head;
    uint32_t k = 0;
    t->call_first[t->calls] = n->index;
    t->call_count[t->calls] = count;
    t->calls++;
    /* The run is exactly `count` linked nodes, then NULL. */
    for (; n != NULL; n = n->next) {
        t->up[t->ups++] = n->index;
        k++;
    }
    if (k != count) {
        t->bad_run = 1;
    }
}

static const struct pacer_rx_chain_ops node_ops = {node_next, node_set_next, node_offer, node_indicate};

/* `n` linked nodes 0..n-1. */
static void nodes(struct node *v, int n)
{
    int i;
    for (i = 0; i < n; i++) {
        v[i].index = i;
        v[i].next = i + 1 < n ? &v[i + 1] : NULL;
    }
}

/* 1 when v[0..n-1] is linked 0, 1, ..., n-1, NULL. */
static int linked_as_made(const struct node *v, int n)
{
    int i;
    for (i = 0; i < n; i++) {
        if (v[i].next != (i + 1 < n ? &v[i + 1] : NULL)) {
            return 0;
        }
    }
    return 1;
}

static void chains(void)
{
    struct node v[200];
    struct chain_test t;
    void *taken;
    void *kept;
    uint32_t count;
    struct node *n;
    int k;
    char take[201];

    /* Split, nothing taken: the chain as it was. */
    memset(&t, 0, sizeof t);
    t.take = "00000";
    nodes(v, 5);
    kept = pacer_rx_split(&v[0], &node_ops, &t, &taken, &count);
    CHECK(kept == &v[0] && taken == NULL && count == 5 && t.offers == 5 && linked_as_made(v, 5));
    for (k = 0; k < 5; k++) {
        CHECK(t.offered[k] == k);
    }
    /* Split, some taken: both in order, each NULL-terminated. */
    memset(&t, 0, sizeof t);
    t.take = "0110100";
    nodes(v, 7);
    kept = pacer_rx_split(&v[0], &node_ops, &t, &taken, &count);
    CHECK(count == 4 && t.offers == 7);
    n = kept;
    CHECK(n == &v[0] && n->next == &v[3] && v[3].next == &v[5] && v[5].next == &v[6] && v[6].next == NULL);
    CHECK(taken == &v[1] && v[1].next == &v[2] && v[2].next == &v[4] && v[4].next == NULL);
    /* Everything taken: nothing kept. */
    memset(&t, 0, sizeof t);
    t.take = "111";
    nodes(v, 3);
    kept = pacer_rx_split(&v[0], &node_ops, &t, &taken, &count);
    CHECK(kept == NULL && count == 0 && taken == &v[0] && v[0].next == &v[1] && v[2].next == NULL);
    /* The first taken, the last kept; the last taken, the rest kept. */
    memset(&t, 0, sizeof t);
    t.take = "10";
    nodes(v, 2);
    kept = pacer_rx_split(&v[0], &node_ops, &t, &taken, &count);
    CHECK(kept == &v[1] && count == 1 && taken == &v[0] && v[0].next == NULL && v[1].next == NULL);
    memset(&t, 0, sizeof t);
    t.take = "001";
    nodes(v, 3);
    kept = pacer_rx_split(&v[0], &node_ops, &t, &taken, &count);
    CHECK(kept == &v[0] && count == 2 && v[1].next == NULL && taken == &v[2] && v[2].next == NULL);
    /* An empty chain. */
    memset(&t, 0, sizeof t);
    t.take = "";
    kept = pacer_rx_split(NULL, &node_ops, &t, &taken, &count);
    CHECK(kept == NULL && taken == NULL && count == 0 && t.offers == 0);

    /* Cannot pend, nothing taken: one call with the whole chain. */
    memset(&t, 0, sizeof t);
    t.take = "00000";
    nodes(v, 5);
    CHECK(pacer_rx_up_in_runs(&v[0], &node_ops, &t) == 5);
    CHECK(t.calls == 1 && t.call_first[0] == 0 && t.call_count[0] == 5 && !t.bad_run && t.offers == 5);
    CHECK(linked_as_made(v, 5));
    /* Runs between taken nodes, each alone; the chain restored. */
    memset(&t, 0, sizeof t);
    t.take = "0110100";
    nodes(v, 7);
    CHECK(pacer_rx_up_in_runs(&v[0], &node_ops, &t) == 4);
    CHECK(t.calls == 3 && !t.bad_run);
    CHECK(t.call_first[0] == 0 && t.call_count[0] == 1);
    CHECK(t.call_first[1] == 3 && t.call_count[1] == 1);
    CHECK(t.call_first[2] == 5 && t.call_count[2] == 2);
    CHECK(t.ups == 4 && t.up[0] == 0 && t.up[1] == 3 && t.up[2] == 5 && t.up[3] == 6);
    CHECK(linked_as_made(v, 7));
    /* All taken: nothing up, the chain as it was. */
    memset(&t, 0, sizeof t);
    t.take = "111";
    nodes(v, 3);
    CHECK(pacer_rx_up_in_runs(&v[0], &node_ops, &t) == 0);
    CHECK(t.calls == 0 && linked_as_made(v, 3));
    CHECK(pacer_rx_up_in_runs(NULL, &node_ops, &t) == 0 && t.offers == 3);

    /* Longer than a segment: every node offered before its segment goes
     * up; a run is cut at the segment's end; links restored. */
    memset(take, '0', sizeof take - 1);
    take[sizeof take - 1] = '\0';
    take[70] = '1';
    memset(&t, 0, sizeof t);
    t.take = take;
    nodes(v, 150);
    CHECK(pacer_rx_up_in_runs(&v[0], &node_ops, &t) == 149);
    CHECK(t.offers == 150 && !t.bad_run && linked_as_made(v, 150));
    CHECK(t.calls == 4);
    CHECK(t.call_first[0] == 0 && t.call_count[0] == 64);
    CHECK(t.call_first[1] == 64 && t.call_count[1] == 6);
    CHECK(t.call_first[2] == 71 && t.call_count[2] == 57);
    CHECK(t.call_first[3] == 128 && t.call_count[3] == 22);
    for (k = 0; k < 149; k++) {
        CHECK(t.up[k] == (k < 70 ? k : k + 1));
    }
    /* Exactly one segment, nothing taken: one call, links as they were. */
    memset(&t, 0, sizeof t);
    t.take = take;
    nodes(v, 64);
    CHECK(pacer_rx_up_in_runs(&v[0], &node_ops, &t) == 64);
    CHECK(t.calls == 1 && t.call_count[0] == 64 && linked_as_made(v, 64));
}

int main(void)
{
    parse();
    match();
    ring();
    sinks();
    ports();
    chains();
    printf("%d checks, %d failed\n", checks, failed);
    return failed == 0 ? 0 : 1;
}
