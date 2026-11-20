/* dasio kernel RX: the decisions, the ring's producer and the chain work
 * (see rx_ring.h). */
#if defined(_KERNEL_MODE)
/* RtlCopyMemory / RtlZeroMemory, and the MSVC intrinsics below (wdm.h
 * declares them; ntddk.h brings it before pacer_core.h's types in the
 * driver's own files). */
#include <wdm.h>
#define rx_copy(to, from, n) RtlCopyMemory((to), (from), (n))
#define rx_zero(to, n) RtlZeroMemory((to), (n))
#else
#include <string.h>
#if defined(_MSC_VER)
#include <intrin.h>
#endif
#define rx_copy(to, from, n) memcpy((to), (from), (n))
#define rx_zero(to, n) memset((to), 0, (n))
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
 * it and never writes the reader's word.
 *
 * MSVC: one branch for the driver and the host test, so the MSVC test
 * job compiles and runs the very operations the x64 driver uses. */
#if defined(_MSC_VER)
static void store_head(uint8_t *base, uint64_t v)
{
    (void)_InterlockedExchange64((volatile __int64 *)(base + PACER_RX_OFF_HEAD), (__int64)v);
}
#if !defined(_M_X64) && !defined(_M_AMD64)
/* CI builds and tests x64 only: on ARM64 the volatile read below has no
 * acquire semantics (/volatile:iso), and nothing here was ever built for
 * it. Add an ARM64 pacer-driver build with an acquire load first. */
#error "kernel RX (rx_ring.c) is built and tested for x64 only"
#endif
static uint64_t load_tail(uint8_t *base)
{
    /* x64: a volatile read has acquire semantics (/volatile:ms, the
     * compiler's default there, in the driver build too). */
    return *(volatile uint64_t *)(base + PACER_RX_OFF_TAIL);
}
static uint32_t load_wake(uint8_t *base)
{
    /* After store_head's full barrier. */
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

int pacer_rx_parse(const uint8_t *hdr, uint32_t avail, uint32_t frame_len, struct pacer_rx_packet *p)
{
    uint32_t at = 12;
    uint16_t type;
    int tags;
    uint32_t ihl;
    uint32_t total;
    uint32_t udp;
    uint32_t udp_len;
    int vlan = 0;

    if (hdr == NULL || avail > frame_len || avail < ETH_HEADER) {
        return 0;
    }
    type = be16(hdr + at);
    for (tags = 0; tags < 2 && (type == ETHERTYPE_VLAN || type == ETHERTYPE_QINQ); tags++) {
        at += 4;
        if (avail < at + 2) {
            return 0;
        }
        /* The tag's VLAN ID: the low 12 bits of its TCI. */
        if ((be16(hdr + at - 2) & 0x0FFFu) != 0) {
            vlan = 1;
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
    rx_copy(p->src_ip, hdr + at + 12, 4);
    rx_copy(p->dst_ip, hdr + at + 16, 4);
    p->src_port = be16(hdr + udp);
    p->dst_port = be16(hdr + udp + 2);
    p->payload_off = udp + UDP_HEADER;
    p->payload_len = udp_len - UDP_HEADER;
    p->flags = 0;
    if (vlan) {
        p->flags |= PACER_RX_PKT_TAGGED;
    }
    if ((hdr[0] & 1u) != 0) {
        p->flags |= PACER_RX_PKT_ETH_GROUP;
    }
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
        return (p->flags & PACER_RX_PKT_ETH_GROUP) == 0 && !is_multicast_or_broadcast(p->dst_ip);
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

int pacer_rx_ring_init(struct pacer_rx_ring *r, uint8_t *base, uint32_t slots, int zeroed)
{
    uint32_t bytes = pacer_rx_ring_bytes(slots);
    if (bytes == 0) {
        return 0;
    }
    if (!zeroed) {
        rx_zero(base, bytes);
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
    rx_copy(slot + 8, p->src_ip, 4);
    rx_copy(slot + 12, p->dst_ip, 4);
    le16(slot + 16, p->src_port);
    le16(slot + 18, p->dst_port);
    le16(slot + 20, p->payload_off);
    le16(slot + 22, p->payload_len);
    le16(slot + 24, len);
    le16(slot + 26, 0);
    le32(slot + 28, (uint32_t)r->head);
    rx_copy(slot + PACER_RX_META_BYTES, frame, len);
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
        /* Not a frame of the flow as it is meant to arrive: counted, and up
         * to tcpip as ever (it drops a bad checksum itself). */
        if ((p->flags & PACER_RX_PKT_TAGGED) != 0) {
            s->tagged++;
            continue;
        }
        if ((p->flags & PACER_RX_PKT_BAD_CSUM) != 0) {
            s->bad_csum++;
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

void pacer_rx_ports_init(struct pacer_rx_ports *ports)
{
    rx_zero(ports, sizeof *ports);
}

/* 1 when a sink's table names `port`. */
static int named(struct pacer_rx_sink *const *sinks, uint32_t count, uint16_t port)
{
    uint32_t i;
    uint32_t e;
    for (i = 0; i < count; i++) {
        for (e = 0; e < sinks[i]->matches && e < PACER_RX_MAX_MATCHES; e++) {
            if (sinks[i]->match[e].port == port) {
                return 1;
            }
        }
    }
    return 0;
}

/* The bit of `port`, set or cleared alone: its byte is written whole, its
 * other bits as they were (a racing reader sees them unchanged). */
static void port_bit(struct pacer_rx_ports *ports, uint16_t port, int on)
{
    volatile uint8_t *b = &ports->bits[port >> 3];
    uint8_t m = (uint8_t)(1u << (port & 7u));
    *b = (uint8_t)(on ? (*b | m) : (*b & (uint8_t)~m));
}

void pacer_rx_ports_build(struct pacer_rx_sink *const *sinks, uint32_t count, struct pacer_rx_ports *ports)
{
    uint16_t recorded[PACER_RX_MAX_PORTS];
    uint32_t n = 0;
    uint32_t distinct = 0;
    uint32_t i;
    uint32_t e;
    uint32_t k;

    /* Every named port's bit first, so none is ever clear while named. */
    for (i = 0; i < count; i++) {
        for (e = 0; e < sinks[i]->matches && e < PACER_RX_MAX_MATCHES; e++) {
            uint16_t port = sinks[i]->match[e].port;
            int seen = 0;
            for (k = 0; k < n; k++) {
                if (recorded[k] == port) {
                    seen = 1;
                }
            }
            if (seen) {
                continue;
            }
            port_bit(ports, port, 1);
            distinct++;
            if (n < PACER_RX_MAX_PORTS) {
                recorded[n++] = port;
            }
        }
    }
    /* Then the recorded ports no table names any more. */
    for (k = 0; k < ports->recorded_count && k < PACER_RX_MAX_PORTS; k++) {
        if (!named(sinks, count, ports->recorded[k])) {
            port_bit(ports, ports->recorded[k], 0);
        }
    }
    for (k = 0; k < n; k++) {
        ports->recorded[k] = recorded[k];
    }
    ports->recorded_count = n;
    *(volatile uint32_t *)&ports->count = distinct;
}

int pacer_rx_ports_has(const struct pacer_rx_ports *ports, uint16_t port)
{
    const volatile uint8_t *b = &ports->bits[port >> 3];
    return (int)((*b >> (port & 7u)) & 1u);
}

void *pacer_rx_split(void *chain, const struct pacer_rx_chain_ops *ops, void *ctx, void **taken,
                     uint32_t *kept_count)
{
    void *kept = NULL;
    void *last_kept = NULL;
    void *last_taken = NULL;
    void *node = chain;
    uint32_t count = 0;
    int relinked = 0;

    *taken = NULL;
    while (node != NULL) {
        void *next = ops->next(node);
        if (ops->offer(ctx, node)) {
            if (last_taken == NULL) {
                *taken = node;
            } else {
                ops->set_next(last_taken, node);
            }
            last_taken = node;
            relinked = 1;
        } else {
            if (last_kept == NULL) {
                kept = node;
            } else if (relinked) {
                ops->set_next(last_kept, node);
            }
            last_kept = node;
            count++;
        }
        node = next;
    }
    if (relinked) {
        ops->set_next(last_taken, NULL);
        if (last_kept != NULL) {
            ops->set_next(last_kept, NULL);
        }
    }
    *kept_count = count;
    return kept;
}

uint32_t pacer_rx_up_in_runs(void *chain, const struct pacer_rx_chain_ops *ops, void *ctx)
{
    uint32_t up = 0;
    void *segment = chain;

    while (segment != NULL) {
        uint8_t taken[PACER_RX_SEGMENT];
        uint32_t n = 0;
        uint32_t i = 0;
        void *node = segment;
        void *after;

        for (; node != NULL && n < PACER_RX_SEGMENT; n++) {
            taken[n] = (uint8_t)(ops->offer(ctx, node) != 0);
            node = ops->next(node);
        }
        after = node;
        node = segment;
        while (i < n) {
            void *head;
            void *last;
            uint32_t run = 0;
            if (taken[i]) {
                node = ops->next(node);
                i++;
                continue;
            }
            head = node;
            last = node;
            while (i < n && !taken[i]) {
                last = node;
                node = ops->next(node);
                i++;
                run++;
            }
            /* `node` is the run's original successor: a taken node or the
             * next segment's first (cut for the call, then restored), or
             * NULL (the chain's end: nothing to cut). */
            if (node != NULL) {
                ops->set_next(last, NULL);
            }
            ops->indicate(ctx, head, run);
            if (node != NULL) {
                ops->set_next(last, node);
            }
            up += run;
        }
        segment = after;
    }
    return up;
}
