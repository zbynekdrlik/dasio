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

static void budget(void)
{
    struct pacer p;

    pacer_init(&p, 64000);
    CHECK(p.budget == 64000 && p.in_flight == 0 && p.queued == 0);

    /* Under the budget everything goes down. */
    CHECK(pacer_offer(&p, 30000) == 1);
    CHECK(pacer_offer(&p, 34000) == 1);
    CHECK(p.in_flight == 64000);
    /* Exactly full: the next byte waits. */
    CHECK(pacer_offer(&p, 1) == 0);
    CHECK(p.queued == 1 && p.queued_bytes == 1 && p.deferred == 1);
    /* While something waits, a new send waits too (order). */
    CHECK(pacer_offer(&p, 1500) == 0);
    CHECK(p.queued == 2 && p.queued_bytes == 1501);
    /* Nothing fits yet. */
    CHECK(pacer_take(&p, 1) == 0);
    /* A completion makes room for the head, then the next. */
    pacer_complete(&p, 30000);
    CHECK(p.in_flight == 34000);
    CHECK(pacer_take(&p, 1) == 1);
    CHECK(pacer_take(&p, 1500) == 1);
    CHECK(p.queued == 0 && p.queued_bytes == 0);
    CHECK(p.in_flight == 35501);
    CHECK(pacer_take(&p, 10) == 0); /* empty queue */
    CHECK(p.admitted == 4);
    CHECK(p.max_queued == 2);
    CHECK(p.max_in_flight == 64000);

    /* One send larger than the budget still moves when nothing is in flight. */
    pacer_init(&p, 1000);
    CHECK(pacer_offer(&p, 65536) == 1);
    CHECK(pacer_offer(&p, 1) == 0);
    pacer_complete(&p, 65536);
    CHECK(p.in_flight == 0);
    CHECK(pacer_take(&p, 1) == 1);
    /* The queue's head too. */
    pacer_init(&p, 1000);
    CHECK(pacer_offer(&p, 900) == 1);
    CHECK(pacer_offer(&p, 5000) == 0);
    CHECK(pacer_take(&p, 5000) == 0);
    pacer_complete(&p, 900);
    CHECK(pacer_take(&p, 5000) == 1);
    CHECK(p.in_flight == 5000);

    /* Completions never take in-flight below zero. */
    pacer_init(&p, 1000);
    CHECK(pacer_offer(&p, 100) == 1);
    pacer_complete(&p, 500);
    CHECK(p.in_flight == 0);

    /* Dropping queued sends (pause, cancel). */
    pacer_init(&p, 100);
    CHECK(pacer_offer(&p, 100) == 1);
    CHECK(pacer_offer(&p, 50) == 0);
    CHECK(pacer_offer(&p, 60) == 0);
    pacer_drop_queued(&p, 2, 110);
    CHECK(p.queued == 0 && p.queued_bytes == 0);
    pacer_drop_queued(&p, 3, 10);
    CHECK(p.queued == 0 && p.queued_bytes == 0);
    CHECK(pacer_offer(&p, 50) == 0); /* still full in flight */
    pacer_drop_queued(&p, 1, 20);
    CHECK(p.queued == 0 && p.queued_bytes == 30);

    /* Priority sends are counted apart and never in flight. */
    pacer_init(&p, 100);
    pacer_note_priority(&p);
    pacer_note_priority(&p);
    CHECK(p.priority_sent == 2 && p.in_flight == 0 && p.admitted == 0);

    /* Budget arithmetic does not overflow near 4 GB. */
    pacer_init(&p, 0xFFFFFFF0u);
    CHECK(pacer_offer(&p, 0xFFFFFFF0u) == 1);
    CHECK(pacer_offer(&p, 0x20u) == 0);
}

int main(void)
{
    classify();
    budget();
    printf("%d checks, %d failed\n", checks, failed);
    return failed == 0 ? 0 : 1;
}
