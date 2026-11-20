/* dasio kernel RX: the decisions and the ring's producer (see rx_ring.h). */
#if defined(_KERNEL_MODE)
/* The interlocked operations, included as the driver's own files do
 * (ntddk.h brings wdm.h before pacer_core.h's types). */
#include <wdm.h>
#elif defined(_MSC_VER)
#include <intrin.h>
#endif

#include "rx_ring.h"

#define ETH_HEADER 14u
#define ETHERTYPE_IPV4 0x0800u
#define ETHERTYPE_VLAN 0x8100u
#define ETHERTYPE_QINQ 0x88A8u
#define IPPROTO_UDP_ 17u
#define UDP_HEADER 8u

/* The shared words of the ring's header. The head store is a full
 * barrier (it orders the slot's bytes before it, and the read of wake
 * after it: no lost wake); the tail load orders the slot writes after
 * it and never writes the reader's word. */
#if defined(_KERNEL_MODE)
static void store_head(uint8_t *base, uint64_t v)
{
    (void)InterlockedExchange64((volatile LONG64 *)(base + PACER_RX_OFF_HEAD), (LONG64)v);
}
static uint64_t load_tail(uint8_t *base)
{
    return (uint64_t)ReadAcquire64((volatile LONG64 *)(base + PACER_RX_OFF_TAIL));
}
static uint32_t load_wake(uint8_t *base)
{
    return (uint32_t)(*(volatile LONG *)(base + PACER_RX_OFF_WAKE));
}
static uint32_t take_wake(uint8_t *base)
{
    return (uint32_t)InterlockedExchange((volatile LONG *)(base + PACER_RX_OFF_WAKE), 0);
}
static void store_detached(uint8_t *base)
{
    (void)InterlockedExchange((volatile LONG *)(base + PACER_RX_OFF_DETACHED), 1);
}
#elif defined(_MSC_VER)
static void store_head(uint8_t *base, uint64_t v)
{
    (void)_InterlockedExchange64((volatile __int64 *)(base + PACER_RX_OFF_HEAD), (__int64)v);
}
static uint64_t load_tail(uint8_t *base)
{
    /* MSVC's volatile read has acquire semantics on x64 (/volatile:ms,
     * its default there; the tests' compiler). */
    return *(volatile uint64_t *)(base + PACER_RX_OFF_TAIL);
}
static uint32_t load_wake(uint8_t *base)
{
    return (uint32_t)(*(volatile long *)(base + PACER_RX_OFF_WAKE));
}
static uint32_t take_wake(uint8_t *base)
{
    return (uint32_t)_InterlockedExchange((volatile long *)(base + PACER_RX_OFF_WAKE), 0);
}
static void store_detached(uint8_t *base)
{
    (void)_InterlockedExchange((volatile long *)(base + PACER_RX_OFF_DETACHED), 1);
}
#else
static void store_head(uint8_t *base, uint64_t v)
{
    __atomic_store_n((uint64_t *)(void *)(base + PACER_RX_OFF_HEAD), v, __ATOMIC_SEQ_CST);
}
static uint64_t load_tail(uint8_t *base)
{
    return __atomic_load_n((uint64_t *)(void *)(base + PACER_RX_OFF_TAIL), __ATOMIC_ACQUIRE);
}
static uint32_t load_wake(uint8_t *base)
{
    return __atomic_load_n((uint32_t *)(void *)(base + PACER_RX_OFF_WAKE), __ATOMIC_SEQ_CST);
}
static uint32_t take_wake(uint8_t *base)
{
    return __atomic_exchange_n((uint32_t *)(void *)(base + PACER_RX_OFF_WAKE), 0u, __ATOMIC_SEQ_CST);
}
static void store_detached(uint8_t *base)
{
    __atomic_store_n((uint32_t *)(void *)(base + PACER_RX_OFF_DETACHED), 1u, __ATOMIC_SEQ_CST);
}
#endif

static uint16_t be16(const uint8_t *b) { return (uint16_t)((b[0] << 8) | b[1]); }

static void le16(uint8_t *p, uint32_t v)
{
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
}

static void le32(uint8_t *p, uint32_t v)
{
    le16(p, v);
    le16(p + 2, v >> 16);
}

static void le64(uint8_t *p, uint64_t v)
{
    le32(p, (uint32_t)v);
    le32(p + 4, (uint32_t)(v >> 32));
}

static void copy_bytes(uint8_t *to, const uint8_t *from, uint32_t n)
{
    uint32_t i;
    for (i = 0; i < n; i++) {
        to[i] = from[i];
    }
}

int pacer_rx_parse(const uint8_t *hdr, uint32_t avail, uint32_t frame_len, struct pacer_rx_packet *p)
{
    uint32_t at = 12;
    uint16_t type;
    int tags;
    uint32_t ihl;
    uint32_t total;
    uint32_t udp;
    uint32_t udp_len;

    if (hdr == NULL || avail > frame_len || avail < ETH_HEADER) {
        return 0;
    }
    type = be16(hdr + at);
    for (tags = 0; tags < 2 && (type == ETHERTYPE_VLAN || type == ETHERTYPE_QINQ); tags++) {
        at += 4;
        if (avail < at + 2) {
            return 0;
        }
        type = be16(hdr + at);
    }
    at += 2;
    if (type != ETHERTYPE_IPV4 || avail < at + 20 || (hdr[at] >> 4) != 4) {
        return 0;
    }
    ihl = (uint32_t)(hdr[at] & 0x0F) * 4u;
    /* MF set or an offset: a fragment, whose UDP header is elsewhere. */
    if (ihl < 20 || hdr[at + 9] != IPPROTO_UDP_ || (be16(hdr + at + 6) & 0x3FFFu) != 0) {
        return 0;
    }
    udp = at + ihl;
    if (avail < udp + UDP_HEADER) {
        return 0;
    }
    total = be16(hdr + at + 2);
    udp_len = be16(hdr + udp + 4);
    if (total < ihl + UDP_HEADER || total > frame_len - at || udp_len < UDP_HEADER || udp_len > total - ihl) {
        return 0;
    }
    copy_bytes(p->src_ip, hdr + at + 12, 4);
    copy_bytes(p->dst_ip, hdr + at + 16, 4);
    p->src_port = be16(hdr + udp);
    p->dst_port = be16(hdr + udp + 2);
    p->payload_off = udp + UDP_HEADER;
    p->payload_len = udp_len - UDP_HEADER;
    return 1;
}

static int is_multicast_or_broadcast(const uint8_t ip[4])
{
    return (ip[0] & 0xF0u) == 0xE0u || (ip[0] == 0xFFu && ip[1] == 0xFFu && ip[2] == 0xFFu && ip[3] == 0xFFu);
}

static int entry_matches(const struct pacer_rx_match_entry *e, const struct pacer_rx_packet *p)
{
    int any = e->addr[0] == 0 && e->addr[1] == 0 && e->addr[2] == 0 && e->addr[3] == 0;
    if (e->port != p->dst_port) {
        return 0;
    }
    if (any) {
        return !is_multicast_or_broadcast(p->dst_ip);
    }
    return e->addr[0] == p->dst_ip[0] && e->addr[1] == p->dst_ip[1] && e->addr[2] == p->dst_ip[2] &&
           e->addr[3] == p->dst_ip[3];
}

int pacer_rx_match(const struct pacer_rx_match_entry *table, uint32_t count, const struct pacer_rx_packet *p)
{
    uint32_t i;
    for (i = 0; i < count && i < PACER_RX_MAX_MATCHES; i++) {
        if (entry_matches(&table[i], p)) {
            return 1;
        }
    }
    return 0;
}

int pacer_rx_match_valid(const struct pacer_rx_match_entry *table, uint32_t count)
{
    uint32_t i;
    if (count > PACER_RX_MAX_MATCHES) {
        return 0;
    }
    for (i = 0; i < count; i++) {
        if (table[i].port == 0) {
            return 0;
        }
    }
    return 1;
}

uint32_t pacer_rx_ring_bytes(uint32_t slots)
{
    if (slots < PACER_RX_MIN_SLOTS || slots > PACER_RX_MAX_SLOTS || (slots & (slots - 1u)) != 0) {
        return 0;
    }
    return PACER_RX_HEADER_BYTES + slots * PACER_RX_SLOT_BYTES;
}

int pacer_rx_ring_init(struct pacer_rx_ring *r, uint8_t *base, uint32_t slots)
{
    uint32_t bytes = pacer_rx_ring_bytes(slots);
    uint32_t i;
    if (bytes == 0) {
        return 0;
    }
    for (i = 0; i < bytes; i++) {
        base[i] = 0;
    }
    le32(base + PACER_RX_OFF_MAGIC, PACER_RX_MAGIC);
    le32(base + PACER_RX_OFF_VERSION, PACER_RX_VERSION);
    le32(base + PACER_RX_OFF_SLOTS, slots);
    le32(base + PACER_RX_OFF_SLOT_BYTES, PACER_RX_SLOT_BYTES);
    le32(base + PACER_RX_OFF_DATA, PACER_RX_HEADER_BYTES);
    r->base = base;
    r->slots = slots;
    r->head = 0;
    r->tail_seen = 0;
    return 1;
}

void pacer_rx_ring_detach(struct pacer_rx_ring *r)
{
    store_detached(r->base);
}

enum pacer_rx_put_result pacer_rx_put(struct pacer_rx_ring *r, const uint8_t *frame, uint32_t len,
                                      const struct pacer_rx_packet *p, uint64_t time_100ns, int *wake)
{
    uint8_t *slot;

    if (frame == NULL || len > PACER_RX_MAX_FRAME) {
        return PACER_RX_TOO_BIG;
    }
    /* The tail is read again only when the ring looks full from the last
     * read. A tail ahead of head or more than a ring behind gives a huge
     * or too large difference: full, whatever the reader wrote. */
    if (r->head - r->tail_seen >= r->slots) {
        uint64_t tail = load_tail(r->base);
        if (r->head - tail >= r->slots) {
            return PACER_RX_FULL;
        }
        r->tail_seen = tail;
    }
    slot = r->base + PACER_RX_HEADER_BYTES + (uint32_t)(r->head & (r->slots - 1u)) * PACER_RX_SLOT_BYTES;
    le64(slot, time_100ns);
    copy_bytes(slot + 8, p->src_ip, 4);
    copy_bytes(slot + 12, p->dst_ip, 4);
    le16(slot + 16, p->src_port);
    le16(slot + 18, p->dst_port);
    le16(slot + 20, p->payload_off);
    le16(slot + 22, p->payload_len);
    le16(slot + 24, len);
    le16(slot + 26, 0);
    le32(slot + 28, (uint32_t)r->head);
    copy_bytes(slot + PACER_RX_META_BYTES, frame, len);
    r->head++;
    store_head(r->base, r->head);
    if (load_wake(r->base) != 0 && take_wake(r->base) != 0) {
        *wake = 1;
    }
    return PACER_RX_DELIVERED;
}

int pacer_rx_set_match(struct pacer_rx_sink *s, const struct pacer_rx_match_entry *table, uint32_t count)
{
    uint32_t i;
    if (!pacer_rx_match_valid(table, count)) {
        return 0;
    }
    for (i = 0; i < count; i++) {
        s->match[i] = table[i];
    }
    s->matches = count;
    return 1;
}

int pacer_rx_wanted(struct pacer_rx_sink *const *sinks, uint32_t count, const struct pacer_rx_packet *p)
{
    uint32_t i;
    for (i = 0; i < count; i++) {
        if (pacer_rx_match(sinks[i]->match, sinks[i]->matches, p)) {
            return 1;
        }
    }
    return 0;
}

int pacer_rx_deliver(struct pacer_rx_sink *const *sinks, uint32_t count, const struct pacer_rx_packet *p,
                     const uint8_t *frame, uint32_t len, uint64_t time_100ns)
{
    uint32_t i;
    int matched = 0;
    int goes_up = 0;

    for (i = 0; i < count; i++) {
        struct pacer_rx_sink *s = sinks[i];
        if (!pacer_rx_match(s->match, s->matches, p)) {
            continue;
        }
        matched = 1;
        s->matched++;
        switch (pacer_rx_put(&s->ring, frame, len, p, time_100ns, &s->wake)) {
        case PACER_RX_DELIVERED:
            s->delivered++;
            break;
        case PACER_RX_FULL:
            s->full++;
            break;
        default:
            /* Nothing could keep it: it goes up, whatever the mode. */
            s->too_big++;
            goes_up = 1;
            break;
        }
        if (!s->steal) {
            goes_up = 1;
        }
    }
    if (!matched || goes_up) {
        return 0;
    }
    for (i = 0; i < count; i++) {
        if (pacer_rx_match(sinks[i]->match, sinks[i]->matches, p)) {
            sinks[i]->stolen++;
        }
    }
    return 1;
}
