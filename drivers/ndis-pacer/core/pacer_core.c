/* dasio NDIS send pacer: the decisions (see pacer_core.h). */
#include "pacer_core.h"

#define ETH_HEADER 14u
#define ETHERTYPE_IPV4 0x0800u
#define ETHERTYPE_IPV6 0x86DDu
#define ETHERTYPE_VLAN 0x8100u
#define ETHERTYPE_QINQ 0x88A8u
#define ETHERTYPE_PTP 0x88F7u
#define IPPROTO_UDP_ 17u
#define TICKS_PER_SECOND 10000000u
#define PTP_EVENT_PORT 319u
#define PTP_GENERAL_PORT 320u

static uint16_t be16(const uint8_t *b) { return (uint16_t)((b[0] << 8) | b[1]); }

static int is_priority_dscp(uint8_t dscp)
{
    return dscp == PACER_DSCP_EF || dscp == PACER_DSCP_CS7;
}

static int is_ptp_port(uint16_t port)
{
    return port == PTP_EVENT_PORT || port == PTP_GENERAL_PORT;
}

enum pacer_class pacer_classify(const uint8_t *frame, uint32_t len)
{
    uint32_t at = 12;
    uint16_t type;
    int tags;

    if (frame == NULL || len < ETH_HEADER) {
        return PACER_OTHER;
    }
    type = be16(frame + at);
    for (tags = 0; tags < 2 && (type == ETHERTYPE_VLAN || type == ETHERTYPE_QINQ); tags++) {
        at += 4;
        if (len < at + 2) {
            return PACER_OTHER;
        }
        type = be16(frame + at);
    }
    at += 2;

    if (type == ETHERTYPE_PTP) {
        return PACER_PRIORITY;
    }
    if (type == ETHERTYPE_IPV4) {
        uint32_t ihl;
        if (len < at + 20 || (frame[at] >> 4) != 4) {
            return PACER_OTHER;
        }
        if (is_priority_dscp((uint8_t)(frame[at + 1] >> 2))) {
            return PACER_PRIORITY;
        }
        ihl = (uint32_t)(frame[at] & 0x0F) * 4u;
        if (ihl < 20 || frame[at + 9] != IPPROTO_UDP_) {
            return PACER_OTHER;
        }
        /* A later fragment carries no UDP header. */
        if ((be16(frame + at + 6) & 0x1FFFu) != 0) {
            return PACER_OTHER;
        }
        if (len < at + ihl + 4) {
            return PACER_OTHER;
        }
        return is_ptp_port(be16(frame + at + ihl + 2)) ? PACER_PRIORITY : PACER_OTHER;
    }
    if (type == ETHERTYPE_IPV6) {
        uint8_t tclass;
        if (len < at + 40 || (frame[at] >> 4) != 6) {
            return PACER_OTHER;
        }
        tclass = (uint8_t)(((frame[at] & 0x0F) << 4) | (frame[at + 1] >> 4));
        if (is_priority_dscp((uint8_t)(tclass >> 2))) {
            return PACER_PRIORITY;
        }
        if (frame[at + 6] != IPPROTO_UDP_ || len < at + 40 + 4) {
            return PACER_OTHER;
        }
        return is_ptp_port(be16(frame + at + 40 + 2)) ? PACER_PRIORITY : PACER_OTHER;
    }
    return PACER_OTHER;
}

void pacer_rate_for(uint64_t link_bps, uint32_t percent, uint32_t burst_bytes, uint32_t tick_100ns,
                    uint32_t min_burst, uint64_t *rate, uint32_t *burst)
{
    uint64_t b;

    if (link_bps == 0 || link_bps == (uint64_t)-1 || percent == 0) {
        *rate = 0;
        *burst = 0;
        return;
    }
    if (percent > 100) {
        percent = 100;
    }
    *rate = link_bps / 8u * percent / 100u;
    b = burst_bytes != 0 ? burst_bytes : *rate * tick_100ns / TICKS_PER_SECOND;
    if (b < min_burst) {
        b = min_burst;
    }
    *burst = b > 0xFFFFFFFFu ? 0xFFFFFFFFu : (uint32_t)b;
}

void pacer_init(struct pacer *p, uint64_t rate, uint32_t burst, uint64_t now)
{
    struct pacer zero = {0};
    *p = zero;
    p->rate = rate;
    p->burst = burst;
    p->tokens = burst;
    p->last = now;
}

void pacer_note_priority(struct pacer *p)
{
    p->priority_sent++;
}

/* Adds the tokens earned since the last refill, never above the burst.
 * The time a fraction of a byte would take is kept for the next refill,
 * so the rate holds however often this runs. A clock that went back adds
 * nothing. */
static void refill(struct pacer *p, uint64_t now)
{
    uint64_t missing;
    uint64_t full;
    uint64_t elapsed;
    uint64_t earned;

    if (now <= p->last || p->rate == 0) {
        return;
    }
    missing = (uint64_t)((int64_t)p->burst - p->tokens);
    /* Ticks until the bucket is full again. */
    full = missing * TICKS_PER_SECOND / p->rate + 1u;
    elapsed = now - p->last;
    if (elapsed >= full) {
        p->tokens = p->burst;
        p->last = now;
        return;
    }
    earned = elapsed * p->rate / TICKS_PER_SECOND;
    p->tokens += (int64_t)earned;
    p->last += earned * TICKS_PER_SECOND / p->rate;
}

static int may_go(struct pacer *p, uint64_t now)
{
    if (p->rate == 0) {
        return 1;
    }
    refill(p, now);
    return p->tokens > 0;
}

static void go_down(struct pacer *p, uint32_t bytes)
{
    if (p->rate != 0) {
        p->tokens -= bytes;
    }
    p->admitted++;
}

int pacer_offer(struct pacer *p, uint32_t bytes, uint64_t now)
{
    if (p->queued == 0 && may_go(p, now)) {
        go_down(p, bytes);
        return 1;
    }
    p->queued++;
    p->queued_bytes += bytes;
    p->deferred++;
    if (p->queued > p->max_queued) {
        p->max_queued = p->queued;
    }
    return 0;
}

int pacer_take(struct pacer *p, uint32_t bytes, uint64_t now)
{
    if (p->queued == 0 || !may_go(p, now)) {
        return 0;
    }
    p->queued--;
    p->queued_bytes = p->queued_bytes >= bytes ? p->queued_bytes - bytes : 0;
    go_down(p, bytes);
    return 1;
}

uint64_t pacer_wait(struct pacer *p, uint64_t now)
{
    uint64_t missing;
    if (p->queued == 0 || p->rate == 0) {
        return 0;
    }
    refill(p, now);
    if (p->tokens > 0) {
        return 0;
    }
    /* Tokens must reach 1 byte. */
    missing = (uint64_t)(1 - p->tokens);
    return (missing * TICKS_PER_SECOND + p->rate - 1u) / p->rate;
}

void pacer_drop_queued(struct pacer *p, uint32_t count, uint64_t bytes)
{
    p->queued = p->queued >= count ? p->queued - count : 0;
    p->queued_bytes = p->queued_bytes >= bytes ? p->queued_bytes - bytes : 0;
}

/* The flag stays set while the taken entries go down, so a pump called
 * from inside one of those sends (an inline completion) returns at once;
 * it is cleared only by a pass that finds nothing due, under the lock. */
void pacer_pump(uint32_t *pumping, const struct pacer_pump_ops *ops, void *ctx)
{
    struct pacer_taken taken[PACER_PUMP_BATCH];
    uint32_t count;
    uint32_t i;

    ops->lock(ctx);
    if (*pumping) {
        ops->unlock(ctx);
        return;
    }
    *pumping = 1;
    for (;;) {
        count = ops->take(ctx, taken, PACER_PUMP_BATCH);
        if (count == 0) {
            *pumping = 0;
            ops->idle(ctx);
            ops->unlock(ctx);
            return;
        }
        ops->unlock(ctx);
        for (i = 0; i < count; i++) {
            ops->send(ctx, &taken[i]);
        }
        ops->lock(ctx);
    }
}

void pacer_split_init(struct pacer_split *s, uint32_t children)
{
    s->pending = children;
    s->status = 0;
}

int pacer_split_done(struct pacer_split *s, int32_t status)
{
    if (s->status == 0) {
        s->status = status;
    }
    if (s->pending > 0) {
        s->pending--;
    }
    return s->pending == 0;
}
