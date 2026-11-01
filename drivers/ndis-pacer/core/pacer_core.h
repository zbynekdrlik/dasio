/* dasio NDIS send pacer: the decisions, free of any kernel type.
 *
 * Windows sends every frame of a single-queue NIC in one FIFO, so a
 * Dante packet waits behind whatever bulk data (a 1.5 MB TCP answer) is
 * already queued for the miniport: up to ~5 ms at 2.5 Gbit/s (dasio#1).
 * Linux avoids this with BQL (a cap on bytes in the NIC) and fq_codel.
 * This filter does the same below TCP/IP: priority frames go down at
 * once; all other frames go down only while the bytes of other traffic
 * the miniport holds stay under a budget, and wait here otherwise.
 *
 * The NDIS glue (filter.c) calls these functions under its own lock. */
#ifndef PACER_CORE_H
#define PACER_CORE_H

#include <stddef.h>
#include <stdint.h>

/* DSCP values that mark priority traffic: Dante audio (EF) and Dante
 * PTP / control (CS7). */
#define PACER_DSCP_EF 46u
#define PACER_DSCP_CS7 56u

enum pacer_class {
    PACER_OTHER = 0,
    PACER_PRIORITY = 1,
};

/* The class of one Ethernet frame from its first `len` bytes: priority
 * for an IPv4/IPv6 packet marked DSCP 46 or 56, a PTP frame (EtherType
 * 0x88F7) or PTP over UDP (destination port 319 or 320); everything else,
 * and anything too short to read, is other. One or two VLAN tags are
 * skipped. */
enum pacer_class pacer_classify(const uint8_t *frame, size_t len);

struct pacer {
    /* Bytes of other traffic the miniport may hold at once. */
    uint32_t budget;
    /* Other bytes sent down and not yet completed. */
    uint32_t in_flight;
    /* Other sends waiting here, in order, and their bytes. */
    uint32_t queued;
    uint64_t queued_bytes;
    /* Counters for the status line. */
    uint64_t priority_sent;
    uint64_t admitted;
    uint64_t deferred;
    uint32_t max_queued;
    uint32_t max_in_flight;
};

void pacer_init(struct pacer *p, uint32_t budget);

/* A priority send: it goes down at once and is never counted against
 * the budget. */
void pacer_note_priority(struct pacer *p);

/* A new other send of `bytes`. Returns 1 when it may go down now (it is
 * then counted in flight), 0 when it must wait at the end of the queue
 * (counted as queued). Order is kept: while anything waits, a new send
 * waits too. One send always goes down when nothing is in flight, so a
 * send larger than the budget still moves. */
int pacer_offer(struct pacer *p, uint32_t bytes);

/* The head of the queue holds `bytes`. Returns 1 when it may go down now
 * (it then leaves the queue and is counted in flight), 0 otherwise. */
int pacer_take(struct pacer *p, uint32_t bytes);

/* The miniport completed an other send of `bytes` that went down. */
void pacer_complete(struct pacer *p, uint32_t bytes);

/* `count` queued sends of `bytes` in all left the queue without going
 * down (pause, cancel). */
void pacer_drop_queued(struct pacer *p, uint32_t count, uint64_t bytes);

#endif
