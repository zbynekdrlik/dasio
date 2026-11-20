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
 *   frame; the driver's 128-byte header read gives the same answer as the
 *   whole frame, and a shorter readable prefix gives 0 or that answer;
 * - pacer_rx_match matches an entry naming the packet's own destination
 *   and nothing on another port; pacer_rx_match_valid refuses more than
 *   16 entries or a port 0;
 * - pacer_rx_put, whatever the reader wrote as tail and wherever head
 *   stands, writes only inside the ring, publishes exactly the frames it
 *   reports delivered and never touches the header's fixed fields.
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
           a->payload_len == b->payload_len;
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
    /* An entry naming the packet's own destination matches. */
    i = count < PACER_RX_MAX_MATCHES ? count : PACER_RX_MAX_MATCHES - 1u;
    memcpy(t[i].addr, p->dst_ip, 4);
    t[i].port = p->dst_port;
    require(pacer_rx_match(t, i + 1u, p) == 1);
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
    b = malloc(bytes);
    require(b != NULL);
    require(pacer_rx_ring_init(&r, b, slots) == 1);
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
    free(b);
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
    return 0;
}
