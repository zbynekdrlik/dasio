/* libFuzzer target for kernel RX (dasio#27, B23): the frame parser, the
 * match table and the ring's producer.
 *
 * The filter runs these on every received frame, in the kernel, and
 * writes into memory the dasio process also writes: a read past a frame
 * or a write past the ring there is a blue screen. libFuzzer hands each
 * input in a heap buffer of exactly its size and the ring is allocated
 * at exactly its size, so AddressSanitizer stops at the first byte past
 * either. Next to memory safety the target checks what each promises:
 *
 * - pacer_rx_parse answers 0 or 1 and reads only its first `avail`
 *   bytes; a packet it reads has its UDP header and payload inside the
 *   frame, and its flags say what the bytes say (a tag with a VLAN ID,
 *   the Ethernet group bit); the driver's 128-byte header read gives the same answer as the
 *   whole frame, and a shorter readable prefix gives 0 or that answer;
 * - pacer_rx_match matches an entry naming the packet's own destination
 *   and nothing on another port; pacer_rx_match_valid refuses more than
 *   16 entries or a port 0;
 * - pacer_rx_put, whatever the reader wrote as tail and wherever head
 *   stands, writes only inside the ring, publishes exactly the frames it
 *   reports delivered and never touches the header's fixed fields;
 * - pacer_rx_deliver, over 1 to 4 sinks (copy or steal, matching or not,
 *   full or not, the packet tagged or with a bad checksum or not), counts
 *   the frame once in each matching sink and keeps it from tcpip exactly
 *   when every matching sink steals and none counted it too big; a tagged
 *   or bad-checksum packet is only counted;
 * - pacer_rx_ports_has answers 1 for every port a table names;
 * - pacer_rx_split and pacer_rx_up_in_runs, over up to 159 nodes with
 *   any taken set: every node offered once, in order; the kept and taken
 *   nodes in order; each indicated run is consecutive kept nodes, NULL-
 *   terminated, indicated only after its whole segment was offered; the
 *   chain linked as it was when up_in_runs returns.
 *
 * Built with clang -fsanitize=fuzzer,address,undefined (CI, Linux). */
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "../core/rx_ring.h"

static void require(int cond)
{
    if (!cond) {
        abort();
    }
}

/* `data[0..len)` copied into a buffer of exactly `len` bytes. */
static uint8_t *exact(const uint8_t *data, size_t len)
{
    uint8_t *b = malloc(len ? len : 1);
    require(b != NULL);
    if (len) {
        memcpy(b, data, len);
    }
    return b;
}

static uint16_t be16(const uint8_t *b) { return (uint16_t)((b[0] << 8) | b[1]); }

static uint64_t get64le(const uint8_t *p)
{
    uint64_t v = 0;
    int i;
    for (i = 7; i >= 0; i--) {
        v = (v << 8) | p[i];
    }
    return v;
}

static void set64le(uint8_t *p, uint64_t v)
{
    int i;
    for (i = 0; i < 8; i++) {
        p[i] = (uint8_t)(v >> (8 * i));
    }
}

static int same(const struct pacer_rx_packet *a, const struct pacer_rx_packet *b)
{
    return memcmp(a->src_ip, b->src_ip, 4) == 0 && memcmp(a->dst_ip, b->dst_ip, 4) == 0 &&
           a->src_port == b->src_port && a->dst_port == b->dst_port && a->payload_off == b->payload_off &&
           a->payload_len == b->payload_len && a->flags == b->flags;
}

/* Returns the parse result on the whole input, `*p` its packet. */
static int parse(const uint8_t *data, size_t size, struct pacer_rx_packet *p)
{
    uint32_t len = (uint32_t)size;
    struct pacer_rx_packet q;
    uint8_t *b;
    int r = pacer_rx_parse(data, len, len, p);
    int r2;
    uint32_t avail;

    require(r == 0 || r == 1);
    if (r) {
        uint32_t udp = p->payload_off - 8u;
        require(p->payload_off >= 14u + 20u + 8u && p->payload_off <= 14u + 8u + 60u + 8u);
        require(p->payload_len <= len - p->payload_off);
        require(be16(data + udp) == p->src_port && be16(data + udp + 2) == p->dst_port);
        /* The flags from the bytes: up to 2 tags before IPv4, a tag with
         * VLAN ID bits is tagged; the group bit of the first byte. */
        {
            uint32_t want = (data[0] & 1u) ? PACER_RX_PKT_ETH_GROUP : 0u;
            uint32_t at = 12;
            int t;
            for (t = 0; t < 2 && (be16(data + at) == 0x8100u || be16(data + at) == 0x88A8u); t++) {
                if ((be16(data + at + 2) & 0x0FFFu) != 0) {
                    want |= PACER_RX_PKT_TAGGED;
                }
                at += 4;
            }
            require(p->flags == want);
        }
        require((uint32_t)be16(data + udp + 4) == p->payload_len + 8u);
    }

    /* What the driver reads: the first 128 bytes at most. */
    avail = len < 128u ? len : 128u;
    b = exact(data, avail);
    r2 = pacer_rx_parse(b, avail, len, &q);
    require(r2 == r);
    require(!r || same(p, &q));
    free(b);

    /* Any readable prefix: 0, or the same packet. */
    if (size > 0) {
        size_t cut = data[0] % (size + 1);
        b = exact(data, cut);
        r2 = pacer_rx_parse(b, (uint32_t)cut, len, &q);
        require(r2 == 0 || (r == 1 && same(p, &q)));
        free(b);
    }
    return r;
}

static void match(const uint8_t *data, size_t size, const struct pacer_rx_packet *p)
{
    struct pacer_rx_match_entry t[20];
    uint32_t count;
    uint32_t i;
    int has_zero_port = 0;

    if (size < 2) {
        return;
    }
    count = data[1] % 20u;
    for (i = 0; i < count; i++) {
        size_t at = 2 + (size_t)i * 6;
        if (at + 6 <= size) {
            memcpy(t[i].addr, data + at, 4);
            t[i].port = be16(data + at + 4);
        } else {
            memset(t[i].addr, 0, 4);
            t[i].port = (uint16_t)(i + 1);
        }
        if (t[i].port == 0) {
            has_zero_port = 1;
        }
    }
    require(pacer_rx_match_valid(t, count) == (count <= PACER_RX_MAX_MATCHES && !has_zero_port));
    {
        int m = pacer_rx_match(t, count, p);
        require(m == 0 || m == 1);
    }
    /* Every entry on another port: no match. */
    for (i = 0; i < count; i++) {
        t[i].port = (uint16_t)(p->dst_port + 1u + (i % 3u));
    }
    require(pacer_rx_match(t, count, p) == 0);
    /* An entry naming the packet's own destination matches, but for
     * destination 0.0.0.0 in a frame to a group MAC: that entry is "any
     * unicast", which needs a unicast MAC. */
    i = count < PACER_RX_MAX_MATCHES ? count : PACER_RX_MAX_MATCHES - 1u;
    memcpy(t[i].addr, p->dst_ip, 4);
    t[i].port = p->dst_port;
    {
        int zero = p->dst_ip[0] == 0 && p->dst_ip[1] == 0 && p->dst_ip[2] == 0 && p->dst_ip[3] == 0;
        int group = (p->flags & PACER_RX_PKT_ETH_GROUP) != 0;
        require(pacer_rx_match(t, i + 1u, p) == !(zero && group));
    }
}

/* One ring buffer of exactly its size per slot count, made once: a fresh
 * half-megabyte block per input (ASan's quarantine) cost most of the run
 * time. */
static uint8_t *ring_buffer(uint32_t slots)
{
    static uint8_t *made[3];
    uint32_t i = slots == PACER_RX_MIN_SLOTS ? 0u : slots == 2u * PACER_RX_MIN_SLOTS ? 1u : 2u;
    if (made[i] == NULL) {
        made[i] = malloc(pacer_rx_ring_bytes(slots));
        require(made[i] != NULL);
    }
    return made[i];
}

static void ring(const uint8_t *data, size_t size, const struct pacer_rx_packet *p)
{
    struct pacer_rx_ring r;
    uint8_t fixed[20];
    uint8_t frame[PACER_RX_MAX_FRAME + 64];
    uint32_t slots;
    uint32_t bytes;
    uint8_t *b;
    uint64_t start;
    uint32_t puts;
    uint32_t k;

    if (size < 20) {
        return;
    }
    slots = PACER_RX_MIN_SLOTS << (data[2] % 3u);
    bytes = pacer_rx_ring_bytes(slots);
    require(bytes == PACER_RX_HEADER_BYTES + slots * PACER_RX_SLOT_BYTES);
    b = ring_buffer(slots);
    /* Zeroed here, or "zeroed by the allocator" (then only the words the
     * checks read are cleared: the slots' old bytes do not matter). */
    if (data[2] & 8u) {
        set64le(b + PACER_RX_OFF_HEAD, 0);
        memset(b + PACER_RX_OFF_WAKE, 0, 4);
        require(pacer_rx_ring_init(&r, b, slots, 1) == 1);
    } else {
        require(pacer_rx_ring_init(&r, b, slots, 0) == 1);
    }
    memcpy(fixed, b, sizeof fixed);

    /* Head anywhere (wrapping included), the cached tail within a ring of
     * it, the reader's tail anything. */
    start = get64le(data + 3);
    r.head = start;
    r.tail_seen = start - (data[11] % (slots + 1u));
    set64le(b + PACER_RX_OFF_TAIL, get64le(data + 12));
    for (k = 0; k < sizeof frame; k++) {
        frame[k] = data[k % size];
    }

    puts = 1u + data[1] % 8u;
    for (k = 0; k < puts; k++) {
        uint64_t before = r.head;
        uint32_t len = ((uint32_t)data[(k + 4) % size] << 4 | data[(k + 5) % size]) % (uint32_t)(sizeof frame);
        int wake = 0;
        enum pacer_rx_put_result res;
        b[PACER_RX_OFF_WAKE] = (uint8_t)(data[(k + 6) % size] & 1u);
        res = pacer_rx_put(&r, frame, len, p, before, &wake);
        if (res == PACER_RX_DELIVERED) {
            uint8_t *slot = b + PACER_RX_HEADER_BYTES + (uint32_t)(before & (slots - 1u)) * PACER_RX_SLOT_BYTES;
            require(len <= PACER_RX_MAX_FRAME);
            require(r.head == before + 1u);
            require(get64le(slot) == before);
            require(memcmp(slot + PACER_RX_META_BYTES, frame, len) == 0);
            require(wake == (int)(data[(k + 6) % size] & 1u));
            require(b[PACER_RX_OFF_WAKE] == 0);
        } else {
            require(res == (len > PACER_RX_MAX_FRAME ? PACER_RX_TOO_BIG : PACER_RX_FULL));
            require(r.head == before && wake == 0);
        }
        require(get64le(b + PACER_RX_OFF_HEAD) == (r.head == start ? 0 : r.head));
        require(r.head - r.tail_seen <= slots);
    }
    require(memcmp(fixed, b, sizeof fixed) == 0);
}

/* Sink k's ring, made and zeroed once (four fresh rings per input cost
 * most of the run time), then emptied for each input: head 0, the
 * reader's tail and wake 0. The slots' old bytes do not matter here. */
static void sink_ring(uint32_t k, struct pacer_rx_ring *r)
{
    static uint8_t *made[4];
    static struct pacer_rx_ring rings[4];
    if (made[k] == NULL) {
        made[k] = malloc(pacer_rx_ring_bytes(PACER_RX_MIN_SLOTS));
        require(made[k] != NULL);
        require(pacer_rx_ring_init(&rings[k], made[k], PACER_RX_MIN_SLOTS, 0) == 1);
    }
    *r = rings[k];
    set64le(made[k] + PACER_RX_OFF_TAIL, 0);
    memset(made[k] + PACER_RX_OFF_WAKE, 0, 4);
}

static void deliver(const uint8_t *data, size_t size, const struct pacer_rx_packet *pkt)
{
    struct pacer_rx_packet q;
    const struct pacer_rx_packet *p = &q;
    static struct pacer_rx_ports ports;
    struct pacer_rx_sink s[4];
    struct pacer_rx_sink *list[4];
    struct pacer_rx_match_entry e;
    uint8_t frame[PACER_RX_MAX_FRAME + 64];
    int matches[4];
    uint32_t count;
    uint32_t len;
    uint32_t k;
    int any = 0;
    int all_steal = 1;
    int too_big;
    int tagged;
    int bad_csum;
    int wanted = 0;
    int may_steal;
    int expect;
    int res;

    if (size < 24) {
        return;
    }
    /* The packet's flags from the input: tagged, a bad checksum, a group
     * MAC (the caller's and the parser's flags). */
    q = *pkt;
    q.flags = data[20] & (PACER_RX_PKT_TAGGED | PACER_RX_PKT_BAD_CSUM | PACER_RX_PKT_ETH_GROUP);
    tagged = (q.flags & PACER_RX_PKT_TAGGED) != 0;
    may_steal = (data[21] & 1u) == 0;
    bad_csum = !tagged && (q.flags & PACER_RX_PKT_BAD_CSUM) != 0;
    count = 1u + data[7] % 4u;
    len = ((uint32_t)data[8] << 4 | data[9]) % (uint32_t)sizeof frame;
    for (k = 0; k < sizeof frame; k++) {
        frame[k] = data[(k + 3) % size];
    }
    too_big = (data[19] & 1u) != 0 || len > PACER_RX_MAX_FRAME;
    for (k = 0; k < count; k++) {
        uint8_t bits = data[10 + k];
        memset(&s[k], 0, sizeof s[k]);
        sink_ring(k, &s[k].ring);
        s[k].steal = bits & 1u;
        /* Bit 1: the table names the packet's own destination; else an
         * address and port from the input. Bit 2: the ring is full. */
        if (bits & 2u) {
            memcpy(e.addr, p->dst_ip, 4);
            e.port = p->dst_port;
        } else {
            memcpy(e.addr, data + 14, 4);
            e.port = (uint16_t)((data[18] << 8) | data[15 + k]);
        }
        (void)pacer_rx_set_match(&s[k], &e, 1);
        if (bits & 4u) {
            s[k].ring.head = PACER_RX_MIN_SLOTS;
        }
        list[k] = &s[k];
        matches[k] = pacer_rx_match(s[k].match, s[k].matches, p);
        wanted |= matches[k];
        if (matches[k] && !tagged && !bad_csum) {
            any = 1;
            if (!s[k].steal) {
                all_steal = 0;
            }
        }
    }
    /* The lock-free port look never hides a sink's port. */
    pacer_rx_ports_init(&ports);
    pacer_rx_ports_build(list, count, &ports);
    for (k = 0; k < count; k++) {
        if (s[k].matches) {
            require(pacer_rx_ports_has(&ports, s[k].match[0].port) == 1);
        }
        if (matches[k]) {
            require(pacer_rx_ports_has(&ports, p->dst_port) == 1);
        }
    }
    require(pacer_rx_wanted(list, count, p) == wanted);
    /* may_steal 0 (copy mode, the chain goes up whole): never kept. */
    res = pacer_rx_deliver(list, count, p, (data[19] & 1u) ? NULL : frame, len, 7, may_steal);
    expect = may_steal && any && all_steal && !too_big;
    require(res == expect);
    for (k = 0; k < count; k++) {
        uint8_t bits = data[10 + k];
        if (!matches[k]) {
            require(s[k].matched == 0 && s[k].delivered == 0 && s[k].full == 0 && s[k].too_big == 0 &&
                    s[k].stolen == 0 && s[k].tagged == 0 && s[k].bad_csum == 0);
            continue;
        }
        require(s[k].tagged == (uint64_t)tagged && s[k].bad_csum == (uint64_t)bad_csum);
        if (tagged || bad_csum) {
            require(s[k].matched == 0 && s[k].delivered == 0 && s[k].full == 0 && s[k].too_big == 0 &&
                    s[k].stolen == 0);
            continue;
        }
        require(s[k].matched == 1);
        require(s[k].too_big == (uint64_t)too_big);
        require(s[k].full == (uint64_t)(!too_big && (bits & 4u) != 0));
        require(s[k].delivered == (uint64_t)(!too_big && (bits & 4u) == 0));
        require(s[k].stolen == (uint64_t)expect);
    }
}

/* Up to 5 sinks of up to 16 entries from the input, `at` onwards, ports
 * from a small range so tables repeat each other. Returns the sinks. */
static uint32_t tables(const uint8_t *data, size_t size, size_t at, uint8_t pick, struct pacer_rx_sink *s,
                       struct pacer_rx_sink **list)
{
    uint32_t count = pick % 6u;
    uint32_t k;
    uint32_t e;
    for (k = 0; k < count; k++) {
        memset(&s[k], 0, sizeof s[k]);
        s[k].matches = at < size ? data[at] % 17u : 0;
        at++;
        for (e = 0; e < s[k].matches; e++) {
            s[k].match[e].port = (uint16_t)(1u + (at < size ? data[at] : e) % 90u);
            at++;
        }
        list[k] = &s[k];
    }
    return count;
}

static int names(struct pacer_rx_sink *const *list, uint32_t count, uint16_t port)
{
    uint32_t k;
    uint32_t e;
    for (k = 0; k < count; k++) {
        for (e = 0; e < list[k]->matches; e++) {
            if (list[k]->match[e].port == port) {
                return 1;
            }
        }
    }
    return 0;
}

/* Every named port is found; after a second build, every port named then
 * is found, and a port only the first tables named is gone (unless the
 * first build had more distinct ports than it records). */
static void ports(const uint8_t *data, size_t size)
{
    static struct pacer_rx_ports pt;
    struct pacer_rx_sink s1[5];
    struct pacer_rx_sink s2[5];
    struct pacer_rx_sink *l1[5] = {NULL, NULL, NULL, NULL, NULL};
    struct pacer_rx_sink *l2[5] = {NULL, NULL, NULL, NULL, NULL};
    uint32_t c1;
    uint32_t c2;
    uint32_t port;
    uint32_t recorded_all;

    if (size < 8) {
        return;
    }
    c1 = tables(data, size, 4, data[3], s1, l1);
    c2 = tables(data, size, size / 2, data[2], s2, l2);
    pacer_rx_ports_init(&pt);
    pacer_rx_ports_build(l1, c1, &pt);
    for (port = 0; port <= 91u; port++) {
        require(pacer_rx_ports_has(&pt, (uint16_t)port) == names(l1, c1, (uint16_t)port));
    }
    {
        int any = 0;
        uint32_t k;
        for (k = 0; k < c1; k++) {
            if (l1[k]->matches != 0) {
                any = 1;
            }
        }
        require((pt.count == 0) == !any);
    }
    /* Up to 64 distinct ports, every one is recorded (count exact). */
    recorded_all = pt.count <= PACER_RX_MAX_PORTS;
    pacer_rx_ports_build(l2, c2, &pt);
    for (port = 0; port <= 91u; port++) {
        int now = names(l2, c2, (uint16_t)port);
        int has = pacer_rx_ports_has(&pt, (uint16_t)port);
        if (now) {
            require(has == 1);
        } else if (recorded_all || !names(l1, c1, (uint16_t)port)) {
            require(has == 0);
        }
    }
    require((pt.count == 0) == (pt.recorded_count == 0));
}

/* The chain work over fake nodes. */
struct node {
    struct node *next;
    uint32_t index;
};

struct chain_fuzz {
    const uint8_t *take; /* bit i: node i is taken */
    uint32_t n;
    uint32_t offers;
    uint32_t offered_next; /* the index the next offer must be */
    uint32_t up_next;      /* the index the next indicated node must be at least */
    uint32_t ups;
};

static int taken_bit(const struct chain_fuzz *c, uint32_t i) { return (c->take[i / 8] >> (i % 8)) & 1; }

static void *node_next(void *n) { return ((struct node *)n)->next; }

static void node_set_next(void *n, void *next) { ((struct node *)n)->next = next; }

static int node_offer(void *ctx, void *v)
{
    struct chain_fuzz *c = ctx;
    struct node *n = v;
    require(n->index == c->offered_next);
    c->offered_next++;
    c->offers++;
    return taken_bit(c, n->index);
}

static void node_indicate(void *ctx, void *head, uint32_t count)
{
    struct chain_fuzz *c = ctx;
    struct node *n = head;
    uint32_t segment_end = (n->index / PACER_RX_SEGMENT + 1u) * PACER_RX_SEGMENT;
    uint32_t k = 0;
    uint32_t first = n->index;
    /* Its whole segment was offered first. */
    require(c->offers >= (segment_end < c->n ? segment_end : c->n));
    require(first >= c->up_next);
    for (; n != NULL; n = n->next) {
        /* Consecutive kept nodes, within one segment. */
        require(n->index == first + k && !taken_bit(c, n->index));
        require(n->index < segment_end);
        k++;
        require(k <= count);
    }
    require(k == count && count > 0);
    c->up_next = first + count;
    c->ups += count;
}

static const struct pacer_rx_chain_ops node_ops = {node_next, node_set_next, node_offer, node_indicate};

static void chains(const uint8_t *data, size_t size)
{
    static struct node v[160];
    uint8_t take[20];
    struct chain_fuzz c;
    uint32_t n;
    uint32_t i;
    uint32_t kept_expect = 0;
    uint32_t kept_count;
    void *taken;
    void *kept;
    struct node *x;

    if (size < 2) {
        return;
    }
    n = data[0] % 160u;
    memset(take, 0, sizeof take);
    memcpy(take, data + 1, size - 1 < sizeof take ? size - 1 : sizeof take);
    for (i = 0; i < n; i++) {
        v[i].index = i;
        v[i].next = i + 1 < n ? &v[i + 1] : NULL;
    }
    memset(&c, 0, sizeof c);
    c.take = take;
    c.n = n;

    /* Cannot pend: every kept node up once, in order; links as made. */
    require(pacer_rx_up_in_runs(n ? &v[0] : NULL, &node_ops, &c) == c.ups);
    require(c.offers == n);
    for (i = 0; i < n; i++) {
        require(v[i].next == (i + 1 < n ? &v[i + 1] : NULL));
        if (!taken_bit(&c, i)) {
            kept_expect++;
        }
    }
    require(c.ups == kept_expect);

    /* Copy mode: every node offered once, in order, no link changed. */
    memset(&c, 0, sizeof c);
    c.take = take;
    c.n = n;
    require(pacer_rx_offer_all(n ? &v[0] : NULL, &node_ops, &c) == n && c.offers == n && c.ups == 0);
    for (i = 0; i < n; i++) {
        require(v[i].next == (i + 1 < n ? &v[i + 1] : NULL));
    }

    /* May pend: kept and taken, each in order and NULL-terminated. */
    memset(&c, 0, sizeof c);
    c.take = take;
    c.n = n;
    kept = pacer_rx_split(n ? &v[0] : NULL, &node_ops, &c, &taken, &kept_count);
    require(c.offers == n && kept_count == kept_expect);
    {
        uint32_t last = 0;
        uint32_t seen = 0;
        for (x = kept; x != NULL; x = x->next) {
            require(!taken_bit(&c, x->index) && (seen == 0 || x->index > last));
            last = x->index;
            seen++;
        }
        require(seen == kept_expect);
        seen = 0;
        for (x = taken; x != NULL; x = x->next) {
            require(taken_bit(&c, x->index) && (seen == 0 || x->index > last));
            last = x->index;
            seen++;
        }
        require(seen == n - kept_expect);
    }
    /* Nothing taken: no link changed. */
    if (kept_expect == n) {
        for (i = 0; i < n; i++) {
            require(v[i].next == (i + 1 < n ? &v[i + 1] : NULL));
        }
    }
}

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size)
{
    struct pacer_rx_packet p;
    if (!parse(data, size, &p)) {
        memset(&p, 0, sizeof p);
        if (size >= 6) {
            memcpy(p.dst_ip, data, 4);
            p.dst_port = be16(data + 4);
        }
    }
    match(data, size, &p);
    ring(data, size, &p);
    deliver(data, size, &p);
    ports(data, size);
    chains(data, size);
    return 0;
}
