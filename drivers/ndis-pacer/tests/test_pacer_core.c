/* Unit tests of the pacer's decisions (no kernel, any C compiler). */
#include <stdio.h>
#include <string.h>

#include "../core/pacer_core.h"

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

/* An Ethernet frame with `tags` VLAN tags, then an IPv4 header with
 * `dscp`, `proto`, fragment field `frag`, then UDP/TCP ports. */
static uint32_t ipv4(uint8_t *f, int tags, uint8_t dscp, uint8_t proto, uint16_t frag, uint16_t dport)
{
    uint32_t at = 12;
    int i;
    memset(f, 0, 128);
    for (i = 0; i < tags; i++) {
        f[at] = i == 0 && tags == 2 ? 0x88 : 0x81;
        f[at + 1] = i == 0 && tags == 2 ? 0xA8 : 0x00;
        at += 4;
    }
    f[at] = 0x08;
    f[at + 1] = 0x00;
    at += 2;
    f[at] = 0x45;
    f[at + 1] = (uint8_t)(dscp << 2);
    f[at + 6] = (uint8_t)(frag >> 8);
    f[at + 7] = (uint8_t)frag;
    f[at + 9] = proto;
    f[at + 20 + 2] = (uint8_t)(dport >> 8);
    f[at + 20 + 3] = (uint8_t)dport;
    return at + 20 + 8;
}

static uint32_t ipv6(uint8_t *f, uint8_t dscp, uint8_t next, uint16_t dport)
{
    uint8_t tclass = (uint8_t)(dscp << 2);
    memset(f, 0, 128);
    f[12] = 0x86;
    f[13] = 0xDD;
    f[14] = (uint8_t)(0x60 | (tclass >> 4));
    f[15] = (uint8_t)(tclass << 4);
    f[14 + 6] = next;
    f[14 + 40 + 2] = (uint8_t)(dport >> 8);
    f[14 + 40 + 3] = (uint8_t)dport;
    return 14 + 40 + 8;
}

static void classify(void)
{
    uint8_t f[128];
    uint32_t n;

    /* Dante audio (EF) and PTP/control (CS7), whatever the protocol. */
    n = ipv4(f, 0, 46, 17, 0, 14336);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    n = ipv4(f, 0, 56, 17, 0, 8700);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    n = ipv4(f, 0, 46, 6, 0, 80);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    /* Unmarked bulk traffic: TCP and plain UDP. */
    n = ipv4(f, 0, 0, 6, 0, 80);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    n = ipv4(f, 0, 0, 17, 0, 443);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    /* Other DSCP values are not priority. */
    n = ipv4(f, 0, 45, 17, 0, 443);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    n = ipv4(f, 0, 47, 17, 0, 443);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    n = ipv4(f, 0, 8, 17, 0, 443);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    /* PTP over UDP, unmarked: event and general ports only. */
    n = ipv4(f, 0, 0, 17, 0, 319);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    n = ipv4(f, 0, 0, 17, 0, 320);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    n = ipv4(f, 0, 0, 17, 0, 318);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    n = ipv4(f, 0, 0, 17, 0, 321);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    /* Port 319 over TCP is not PTP. */
    n = ipv4(f, 0, 0, 6, 0, 319);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    /* A later fragment has no UDP header: its bytes are not a port. */
    n = ipv4(f, 0, 0, 17, 0x0010, 319);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    /* The first fragment (MF set, offset 0) still has its header. */
    n = ipv4(f, 0, 0, 17, 0x2000, 319);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    /* VLAN and QinQ tags are skipped. */
    n = ipv4(f, 1, 46, 17, 0, 1);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    n = ipv4(f, 2, 46, 17, 0, 1);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    n = ipv4(f, 1, 0, 6, 0, 80);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    /* PTP over Ethernet. */
    memset(f, 0, sizeof f);
    f[12] = 0x88;
    f[13] = 0xF7;
    CHECK(pacer_classify(f, 60) == PACER_PRIORITY);
    /* IPv6: traffic class and PTP ports. */
    n = ipv6(f, 46, 17, 4000);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    n = ipv6(f, 56, 6, 80);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    n = ipv6(f, 0, 6, 80);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    n = ipv6(f, 0, 17, 319);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    n = ipv6(f, 0, 17, 320);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    n = ipv6(f, 0, 17, 5353);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    n = ipv6(f, 0, 6, 319);
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    /* Too short or malformed: other, never a read past `len`. */
    n = ipv4(f, 0, 46, 17, 0, 1);
    CHECK(pacer_classify(f, 13) == PACER_OTHER);
    CHECK(pacer_classify(f, 33) == PACER_OTHER);
    CHECK(pacer_classify(f, 34) == PACER_PRIORITY);
    CHECK(pacer_classify(NULL, 64) == PACER_OTHER);
    n = ipv4(f, 0, 0, 17, 0, 319);
    CHECK(pacer_classify(f, 14 + 20 + 3) == PACER_OTHER);
    CHECK(pacer_classify(f, 14 + 20 + 4) == PACER_PRIORITY);
    n = ipv4(f, 1, 46, 17, 0, 1);
    CHECK(pacer_classify(f, 17) == PACER_OTHER);
    n = ipv4(f, 0, 46, 17, 0, 1);
    f[14] = 0x65; /* version 6 in an IPv4 frame */
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    n = ipv4(f, 0, 0, 17, 0, 319);
    f[14] = 0x44; /* header length 16 bytes: invalid */
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    n = ipv4(f, 0, 0, 17, 0, 319);
    f[14] = 0x46; /* 24-byte header: the port moves 4 bytes on */
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    f[14 + 24 + 2] = 0x01;
    f[14 + 24 + 3] = 0x3F;
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);
    n = ipv6(f, 46, 17, 1);
    CHECK(pacer_classify(f, 14 + 39) == PACER_OTHER);
    n = ipv6(f, 0, 17, 319);
    CHECK(pacer_classify(f, 14 + 40 + 3) == PACER_OTHER);
    f[14] = 0x40; /* version 4 in an IPv6 frame */
    CHECK(pacer_classify(f, n) == PACER_OTHER);
    /* ARP and other EtherTypes. */
    memset(f, 0, sizeof f);
    f[12] = 0x08;
    f[13] = 0x06;
    CHECK(pacer_classify(f, 60) == PACER_OTHER);
}

/* 100 ns units. */
#define US 10u
#define MS 10000u

static void pacing(void)
{
    struct pacer p;
    /* 100 MB/s = 100 bytes per microsecond, burst 10 000 bytes. */
    const uint64_t rate = 100000000u;

    pacer_init(&p, rate, 10000, 1000);
    CHECK(p.rate == rate && p.burst == 10000 && p.tokens == 10000 && p.last == 1000);
    CHECK(p.queued == 0 && p.admitted == 0);

    /* The burst goes down at once. */
    CHECK(pacer_offer(&p, 6000, 1000) == 1);
    CHECK(pacer_offer(&p, 4000, 1000) == 1);
    CHECK(p.tokens == 0);
    /* No tokens left: the next send waits. */
    CHECK(pacer_offer(&p, 1500, 1000) == 0);
    CHECK(p.queued == 1 && p.queued_bytes == 1500 && p.deferred == 1);
    /* While something waits, a new send waits too (order), even with tokens. */
    CHECK(pacer_offer(&p, 100, 1000 + 100 * US) == 0);
    CHECK(p.queued == 2 && p.queued_bytes == 1600);
    CHECK(p.max_queued == 2);
    /* 100 us later 10 000 bytes were earned: the head goes, then the next. */
    CHECK(pacer_take(&p, 1500, 1000 + 100 * US) == 1);
    CHECK(p.tokens == 10000 - 1500);
    CHECK(pacer_take(&p, 100, 1000 + 100 * US) == 1);
    CHECK(p.queued == 0 && p.queued_bytes == 0);
    CHECK(pacer_take(&p, 100, 1000 + 100 * US) == 0); /* empty */
    CHECK(p.admitted == 4);

    /* Tokens never exceed the burst, however long the pause. */
    pacer_init(&p, rate, 10000, 0);
    CHECK(pacer_offer(&p, 1, 0) == 1);
    CHECK(pacer_offer(&p, 1, 3600u * 1000u * MS) == 1);
    CHECK(p.tokens == 10000 - 1);

    /* A send larger than the tokens left still goes; the debt is paid by
     * waiting: 60 000 bytes at 100 B/us leave tokens at -50 000. */
    pacer_init(&p, rate, 10000, 0);
    CHECK(pacer_offer(&p, 60000, 0) == 1);
    CHECK(p.tokens == -50000);
    CHECK(pacer_offer(&p, 1, 0) == 0);
    /* Until tokens reach 1 byte: 50 001 bytes = 500.01 us -> 5001 ticks. */
    CHECK(pacer_wait(&p, 0) == 5001);
    CHECK(pacer_take(&p, 1, 5000) == 0);
    CHECK(pacer_wait(&p, 5000) == 1);
    CHECK(pacer_take(&p, 1, 5001) == 1);
    CHECK(pacer_wait(&p, 5001) == 0); /* nothing waits */

    /* Waiting with tokens left: no wait. */
    pacer_init(&p, rate, 10000, 0);
    CHECK(pacer_offer(&p, 10000, 0) == 1);
    CHECK(pacer_offer(&p, 10, 0) == 0);
    CHECK(pacer_wait(&p, 0) == 1);
    CHECK(pacer_wait(&p, 1) == 0);

    /* A clock that goes back earns nothing and does not break. */
    pacer_init(&p, rate, 10000, 5000);
    CHECK(pacer_offer(&p, 10000, 5000) == 1);
    CHECK(pacer_offer(&p, 10, 4000) == 0);
    CHECK(p.tokens == 0 && p.last == 5000);

    /* Rate 0: no pacing, everything goes, tokens untouched. */
    pacer_init(&p, 0, 0, 0);
    CHECK(pacer_offer(&p, 1000000, 0) == 1);
    CHECK(pacer_offer(&p, 1000000, 0) == 1);
    CHECK(p.tokens == 0 && p.admitted == 2);
    CHECK(pacer_wait(&p, 0) == 0);

    /* Earning is proportional: 25 us at 100 B/us = 2500 bytes. */
    pacer_init(&p, rate, 10000, 0);
    CHECK(pacer_offer(&p, 10000, 0) == 1);
    CHECK(pacer_offer(&p, 2400, 25 * US) == 1);
    CHECK(p.tokens == 100);

    /* A realistic link: 2.5 Gbit/s at 85 % (265 625 000 B/s), drained by a
     * 0.5 ms timer (Windows' timer granularity). The burst must cover one
     * tick (rate x 0.5 ms = 132 812 bytes) or the rate falls with the
     * timer: a 1.5 MB answer then leaves in ~5.6 ms, and at most one tick
     * of it sits ahead of a priority frame. */
    {
        const uint64_t r = 265625000u;
        const uint32_t tick = 5000; /* 0.5 ms */
        uint64_t t = 0;
        uint32_t sent = 0;
        int i;
        pacer_init(&p, r, 132812, 0);
        for (i = 0; i < 1000; i++) {
            if (pacer_offer(&p, 1514, 0)) {
                sent++;
            }
        }
        CHECK(sent == 88); /* 132 812 / 1514 rounded up */
        while (p.queued > 0 && t < 100u * MS) {
            t += tick;
            while (pacer_take(&p, 1514, t)) {
            }
        }
        CHECK(p.queued == 0);
        CHECK(t >= 5u * MS && t <= 6u * MS);

        /* A burst below one tick throttles the rate to burst per tick:
         * 32 KiB per 0.5 ms, so the same answer takes ~23 ms. */
        pacer_init(&p, r, 32768, 0);
        for (i = 0; i < 1000; i++) {
            (void)pacer_offer(&p, 1514, 0);
        }
        t = 0;
        while (p.queued > 0 && t < 100u * MS) {
            t += tick;
            while (pacer_take(&p, 1514, t)) {
            }
        }
        CHECK(p.queued == 0);
        CHECK(t >= 20u * MS);
    }

    /* Dropping queued sends (pause, cancel). */
    pacer_init(&p, rate, 100, 0);
    CHECK(pacer_offer(&p, 100, 0) == 1);
    CHECK(pacer_offer(&p, 50, 0) == 0);
    CHECK(pacer_offer(&p, 60, 0) == 0);
    pacer_drop_queued(&p, 2, 110);
    CHECK(p.queued == 0 && p.queued_bytes == 0);
    pacer_drop_queued(&p, 3, 10);
    CHECK(p.queued == 0 && p.queued_bytes == 0);
    CHECK(pacer_offer(&p, 50, 0) == 0);
    pacer_drop_queued(&p, 1, 20);
    CHECK(p.queued == 0 && p.queued_bytes == 30);

    /* Priority sends are counted apart and use no tokens. */
    pacer_init(&p, rate, 100, 0);
    pacer_note_priority(&p);
    pacer_note_priority(&p);
    CHECK(p.priority_sent == 2 && p.tokens == 100 && p.admitted == 0);
}

/* A model of the driver around the pump: a queue of numbered sends, a
 * token count that lets `due` of them go per take, and a miniport that
 * completes each send inline, which calls the pump again from inside the
 * send (dasio#1: the HIL 2 / 3c double faults, a kernel stack overflow). */
struct model {
    const struct pacer_pump_ops *ops;
    uint32_t pumping;
    uint32_t locked;
    uint32_t next;     /* next entry to take */
    uint32_t count;    /* entries in all */
    uint32_t due;      /* entries a take may move, then 0 until refilled */
    uint32_t refill;   /* `due` after each send (time passes) */
    uint32_t sent;
    uint32_t in_order;
    uint32_t depth;
    uint32_t max_depth;
    uint32_t idles;
    uint32_t lock_errors;
};

static void m_lock(void *ctx)
{
    struct model *m = ctx;
    if (m->locked) {
        m->lock_errors++;
    }
    m->locked = 1;
}

static void m_unlock(void *ctx)
{
    struct model *m = ctx;
    if (!m->locked) {
        m->lock_errors++;
    }
    m->locked = 0;
}

static uint32_t m_take(void *ctx, struct pacer_taken *out, uint32_t max)
{
    struct model *m = ctx;
    uint32_t n = 0;
    if (!m->locked) {
        m->lock_errors++;
    }
    while (n < max && m->due > 0 && m->next < m->count) {
        out[n].item = (void *)(uintptr_t)(m->next + 1);
        out[n].flags = m->next;
        m->next++;
        m->due--;
        n++;
    }
    return n;
}

static void m_idle(void *ctx)
{
    struct model *m = ctx;
    if (!m->locked) {
        m->lock_errors++;
    }
    m->idles++;
}

static void m_send(void *ctx, const struct pacer_taken *t)
{
    struct model *m = ctx;
    if (m->locked) {
        m->lock_errors++;
    }
    if ((uintptr_t)t->item == (uintptr_t)m->sent + 1 && t->flags == m->sent) {
        m->in_order++;
    }
    m->sent++;
    m->due += m->refill;
    /* The miniport completes the send inline: the completion pumps. */
    m->depth++;
    if (m->depth > m->max_depth) {
        m->max_depth = m->depth;
    }
    pacer_pump(&m->pumping, m->ops, m);
    m->depth--;
}

static const struct pacer_pump_ops model_ops = {m_lock, m_unlock, m_take, m_idle, m_send};

static void pump(void)
{
    struct model m;
    uint32_t refill;

    /* Tokens for one send at a time, refilled by each send: every take
     * finds one due entry, the case where a pump started from inside a
     * send would take the next and recurse once per queued entry. */
    for (refill = 0; refill <= 1; refill++) {
        memset(&m, 0, sizeof m);
    m.ops = &model_ops;
        m.ops = &model_ops;
        m.count = 200;
        m.due = 1;
        m.refill = refill;
        pacer_pump(&m.pumping, &model_ops, &m);
        CHECK(m.sent == (refill ? 200u : 1u));
        CHECK(m.in_order == m.sent);
        /* The pump inside a send returns at once: one level, whatever the
         * queue holds. */
        CHECK(m.max_depth == 1);
        CHECK(m.pumping == 0);
        CHECK(m.idles >= 1);
        CHECK(m.lock_errors == 0);
        CHECK(m.locked == 0);
    }

    /* Many due at once: whole batches, in order, then the rest. */
    memset(&m, 0, sizeof m);
    m.ops = &model_ops;
    m.count = 3 * PACER_PUMP_BATCH + 5;
    m.due = m.count;
    pacer_pump(&m.pumping, &model_ops, &m);
    CHECK(m.sent == m.count && m.in_order == m.count);
    CHECK(m.max_depth == 1 && m.pumping == 0 && m.lock_errors == 0);

    /* Nothing waits: no send, the caller still gets its idle (timer). */
    memset(&m, 0, sizeof m);
    m.ops = &model_ops;
    pacer_pump(&m.pumping, &model_ops, &m);
    CHECK(m.sent == 0 && m.idles == 1 && m.pumping == 0);

    /* One runs: a second call sends nothing and changes nothing. */
    memset(&m, 0, sizeof m);
    m.ops = &model_ops;
    m.count = 5;
    m.due = 5;
    m.pumping = 1;
    pacer_pump(&m.pumping, &model_ops, &m);
    CHECK(m.sent == 0 && m.idles == 0 && m.pumping == 1 && m.lock_errors == 0);
}

static void rates(void)
{
    uint64_t rate = 1;
    uint32_t burst = 1;

    /* 85 % of 2.5 Gbit/s, one 0.5 ms tick of it. */
    pacer_rate_for(2500000000u, 85, 0, 5000, 16384, &rate, &burst);
    CHECK(rate == 265625000u && burst == 132812u);
    /* A set burst wins over the tick; never below the minimum. */
    pacer_rate_for(2500000000u, 50, 32768, 5000, 16384, &rate, &burst);
    CHECK(rate == 156250000u && burst == 32768u);
    pacer_rate_for(2500000000u, 50, 1000, 5000, 16384, &rate, &burst);
    CHECK(burst == 16384u);
    pacer_rate_for(100000000u, 85, 0, 5000, 16384, &rate, &burst);
    CHECK(rate == 10625000u && burst == 16384u);
    /* Above 100 % is the link. */
    pacer_rate_for(1000000000u, 250, 0, 5000, 0, &rate, &burst);
    CHECK(rate == 125000000u && burst == 62500u);
    /* No pacing: 0 %, a zero or unknown link speed. */
    pacer_rate_for(2500000000u, 0, 32768, 5000, 16384, &rate, &burst);
    CHECK(rate == 0 && burst == 0);
    rate = burst = 1;
    pacer_rate_for(0, 85, 0, 5000, 16384, &rate, &burst);
    CHECK(rate == 0 && burst == 0);
    rate = burst = 1;
    pacer_rate_for(UINT64_MAX, 85, 0, 5000, 16384, &rate, &burst);
    CHECK(rate == 0 && burst == 0);
    /* A burst beyond 32 bits is capped. */
    pacer_rate_for(UINT64_MAX - 1, 100, 0, 10000000, 0, &rate, &burst);
    CHECK(burst == UINT32_MAX);
}

static void split(void)
{
    struct pacer_split s;

    /* All children succeed: the parent completes once, with success. */
    pacer_split_init(&s, 3);
    CHECK(pacer_split_done(&s, 0) == 0);
    CHECK(pacer_split_done(&s, 0) == 0);
    CHECK(pacer_split_done(&s, 0) == 1);
    CHECK(s.status == 0);

    /* The first failure is the parent's status; later ones do not replace it. */
    pacer_split_init(&s, 3);
    CHECK(pacer_split_done(&s, 0) == 0);
    CHECK(pacer_split_done(&s, -5) == 0);
    CHECK(pacer_split_done(&s, -7) == 1);
    CHECK(s.status == -5);

    /* One child. */
    pacer_split_init(&s, 1);
    CHECK(pacer_split_done(&s, -9) == 1 && s.status == -9);

    /* A completion past the last does not wrap the count. */
    CHECK(pacer_split_done(&s, 0) == 1 && s.pending == 0);
}

static uint64_t be64(const uint8_t *p)
{
    uint64_t v = 0;
    int i;
    for (i = 0; i < 8; i++) {
        v = (v << 8) | p[i];
    }
    return v;
}

static void ktx(void)
{
    uint8_t f[PACER_KTX_HEADER + PACER_KTX_MAX_PAYLOAD + 8];
    struct pacer_ktx_addr a = {
        {0x01, 0x02, 0x03, 0x04, 0x05, 0x06},
        {0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f},
        {192, 168, 0, 199},
        {192, 168, 0, 1},
        0x1234,
        0x5678,
        0,
    };
    uint32_t n, i, sum;
    /* RFC 1071's classic example header: checksum 0xb861. */
    static const uint8_t hdr[20] = {0x45, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11,
                                    0x00, 0x00, 0xc0, 0xa8, 0x00, 0x01, 0xc0, 0xa8, 0x00, 0xc7};

    CHECK(pacer_ipv4_checksum(hdr, 20) == 0xb861);

    /* A frame whose IPv4 header is that example (total length 0x73 = 115:
     * 20 + 8 + 87 payload, identification 0, DF, TTL 64, UDP). */
    memset(f, 0xEE, sizeof f);
    n = pacer_ktx_frame(f, &a, 87, 0, 0x0102030405060708ull, 0x1112131415161718ull);
    CHECK(n == PACER_KTX_HEADER + 87);
    CHECK(memcmp(f, a.dst_mac, 6) == 0 && memcmp(f + 6, a.src_mac, 6) == 0);
    CHECK(f[12] == 0x08 && f[13] == 0x00);
    CHECK(memcmp(f + 14, hdr, 10) == 0);
    CHECK(f[24] == 0xb8 && f[25] == 0x61);
    CHECK(memcmp(f + 26, hdr + 12, 8) == 0);
    /* UDP: ports, length 8 + 87, checksum 0. */
    CHECK(f[34] == 0x56 && f[35] == 0x78 && f[36] == 0x12 && f[37] == 0x34);
    CHECK(f[38] == 0 && f[39] == 95);
    CHECK(f[40] == 0 && f[41] == 0);
    /* Payload: sequence, time, then zeros; nothing written past the frame. */
    CHECK(be64(f + 42) == 0x0102030405060708ull);
    CHECK(be64(f + 50) == 0x1112131415161718ull);
    sum = 0;
    for (i = 58; i < n; i++) {
        sum += f[i];
    }
    CHECK(sum == 0);
    CHECK(f[n] == 0xEE);
    /* The whole header sums to 0xffff (its checksum checks). */
    CHECK(pacer_ipv4_checksum(f + 14, 20) == 0);

    /* DSCP and identification land in the header, and the frame is then
     * priority to the pacer. */
    a.dscp = PACER_DSCP_EF;
    n = pacer_ktx_frame(f, &a, 200, 0xBEEF, 7, 9);
    CHECK(n == PACER_KTX_HEADER + 200);
    CHECK(f[15] == (uint8_t)(PACER_DSCP_EF << 2));
    CHECK(f[18] == 0xBE && f[19] == 0xEF);
    CHECK(f[16] == 0 && f[17] == 228);
    CHECK(pacer_ipv4_checksum(f + 14, 20) == 0);
    CHECK(pacer_classify(f, n) == PACER_PRIORITY);

    /* Payload bounds. */
    memset(f, 0xEE, sizeof f);
    CHECK(pacer_ktx_frame(f, &a, PACER_KTX_MIN_PAYLOAD - 1, 0, 0, 0) == 0);
    CHECK(pacer_ktx_frame(f, &a, PACER_KTX_MAX_PAYLOAD + 1, 0, 0, 0) == 0);
    CHECK(f[0] == 0xEE);
    CHECK(pacer_ktx_frame(f, &a, PACER_KTX_MIN_PAYLOAD, 0, 0, 0) == PACER_KTX_HEADER + PACER_KTX_MIN_PAYLOAD);
    CHECK(pacer_ktx_frame(f, &a, PACER_KTX_MAX_PAYLOAD, 0, 0, 0) == 1514);

    /* An odd header length sums its last byte as the high half. */
    {
        static const uint8_t odd[3] = {0x01, 0x02, 0x03};
        CHECK(pacer_ipv4_checksum(odd, 3) == (uint16_t)~(0x0102 + 0x0300));
    }
    /* The carry folds back in. */
    {
        static const uint8_t ff[4] = {0xff, 0xff, 0x00, 0x02};
        CHECK(pacer_ipv4_checksum(ff, 4) == (uint16_t)~0x0002);
    }

    /* Due frames: 250 us apart from start 1000. */
    CHECK(pacer_ktx_due(1000, 999, 2500, 0, 100) == 0);
    CHECK(pacer_ktx_due(1000, 1000, 2500, 0, 100) == 1);
    CHECK(pacer_ktx_due(1000, 1000, 2500, 1, 100) == 0);
    CHECK(pacer_ktx_due(1000, 3499, 2500, 1, 100) == 0);
    CHECK(pacer_ktx_due(1000, 3500, 2500, 1, 100) == 1);
    CHECK(pacer_ktx_due(1000, 11000, 2500, 0, 100) == 5);
    CHECK(pacer_ktx_due(1000, 11000, 2500, 2, 100) == 3);
    /* A late fire (30 ms) catches up, capped. */
    CHECK(pacer_ktx_due(1000, 1000 + 300000, 2500, 1, 1000) == 120);
    CHECK(pacer_ktx_due(1000, 1000 + 300000, 2500, 1, 16) == 16);
    /* More sent than due, or no interval: nothing. */
    CHECK(pacer_ktx_due(1000, 11000, 2500, 9, 100) == 0);
    CHECK(pacer_ktx_due(1000, 11000, 0, 0, 100) == 0);
}

int main(void)
{
    classify();
    pacing();
    rates();
    split();
    pump();
    ktx();
    printf("%d checks, %d failed\n", checks, failed);
    return failed == 0 ? 0 : 1;
}
