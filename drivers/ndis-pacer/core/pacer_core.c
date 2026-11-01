/* dasio NDIS send pacer: the decisions (see pacer_core.h). */
#include "pacer_core.h"

#define ETH_HEADER 14u
#define ETHERTYPE_IPV4 0x0800u
#define ETHERTYPE_IPV6 0x86DDu
#define ETHERTYPE_VLAN 0x8100u
#define ETHERTYPE_QINQ 0x88A8u
#define ETHERTYPE_PTP 0x88F7u
#define IPPROTO_UDP_ 17u
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

enum pacer_class pacer_classify(const uint8_t *frame, size_t len)
{
    size_t at = 12;
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
        size_t ihl;
        if (len < at + 20 || (frame[at] >> 4) != 4) {
            return PACER_OTHER;
        }
        if (is_priority_dscp((uint8_t)(frame[at + 1] >> 2))) {
            return PACER_PRIORITY;
        }
        ihl = (size_t)(frame[at] & 0x0F) * 4u;
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

void pacer_init(struct pacer *p, uint32_t budget)
{
    struct pacer zero = {0};
    *p = zero;
    p->budget = budget;
}

void pacer_note_priority(struct pacer *p)
{
    p->priority_sent++;
}

static int fits(const struct pacer *p, uint32_t bytes)
{
    return p->in_flight == 0 || (uint64_t)p->in_flight + bytes <= p->budget;
}

static void go_down(struct pacer *p, uint32_t bytes)
{
    p->in_flight += bytes;
    p->admitted++;
    if (p->in_flight > p->max_in_flight) {
        p->max_in_flight = p->in_flight;
    }
}

int pacer_offer(struct pacer *p, uint32_t bytes)
{
    if (p->queued == 0 && fits(p, bytes)) {
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

int pacer_take(struct pacer *p, uint32_t bytes)
{
    if (p->queued == 0 || !fits(p, bytes)) {
        return 0;
    }
    p->queued--;
    p->queued_bytes = p->queued_bytes >= bytes ? p->queued_bytes - bytes : 0;
    go_down(p, bytes);
    return 1;
}

void pacer_complete(struct pacer *p, uint32_t bytes)
{
    p->in_flight = p->in_flight >= bytes ? p->in_flight - bytes : 0;
}

void pacer_drop_queued(struct pacer *p, uint32_t count, uint64_t bytes)
{
    p->queued = p->queued >= count ? p->queued - count : 0;
    p->queued_bytes = p->queued_bytes >= bytes ? p->queued_bytes - bytes : 0;
}
