/* dasio NDIS send pacer: the decisions, free of any kernel type.
 *
 * Windows sends every frame of a single-queue NIC in one FIFO, so a
 * Dante packet waits behind whatever bulk data (a 1.5 MB TCP answer) is
 * already queued for the NIC: up to ~5 ms at 2.5 Gbit/s (dasio#1). Linux
 * avoids this with BQL and fq_codel. The Realtek miniport takes a whole
 * burst and completes it before it is on the wire (measured), so a cap on
 * bytes "in flight" bounds nothing; this filter paces in time instead:
 * priority frames go down at once, other frames at a rate below the
 * link's with a small burst, and wait here otherwise.
 *
 * The NDIS glue (filter.c) calls these functions under its own lock. */
#ifndef PACER_CORE_H
#define PACER_CORE_H

#if defined(_KERNEL_MODE)
/* The driver: kernel types, no user-mode C library headers. */
#include <ntdef.h>
typedef UCHAR uint8_t;
typedef USHORT uint16_t;
typedef ULONG uint32_t;
typedef ULONG64 uint64_t;
typedef LONG64 int64_t;
#else
#include <stddef.h>
#include <stdint.h>
#endif

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
enum pacer_class pacer_classify(const uint8_t *frame, uint32_t len);

struct pacer {
    /* Other traffic may go down at `rate` bytes per second, with at most
     * `burst` bytes at once: the miniport is never handed more than that
     * ahead of a priority frame. 0 = no pacing (everything goes down). */
    uint64_t rate;
    uint32_t burst;
    /* Bytes that may go down now; below zero after a send larger than
     * what was there (the debt is paid before the next send). */
    int64_t tokens;
    /* Time of the last refill, 100 ns units (the caller's clock). */
    uint64_t last;
    /* Other sends waiting here, in order, and their bytes. */
    uint32_t queued;
    uint64_t queued_bytes;
    /* Counters for the status line. */
    uint64_t priority_sent;
    uint64_t admitted;
    uint64_t deferred;
    uint32_t max_queued;
};

/* The rate (bytes/s) and burst (bytes) for a link of `link_bps` bits/s:
 * `percent` of the link (0 = no pacing; above 100 counts as 100), and
 * `burst_bytes`, or when that is 0 the bytes of one `tick_100ns` at the
 * rate; never below `min_burst`. A zero or unknown (all ones) link speed
 * means no pacing: rate and burst 0. */
void pacer_rate_for(uint64_t link_bps, uint32_t percent, uint32_t burst_bytes, uint32_t tick_100ns,
                    uint32_t min_burst, uint64_t *rate, uint32_t *burst);

/* `rate` bytes per second (0 = no pacing), `burst` bytes, starting full at
 * time `now` (100 ns units). */
void pacer_init(struct pacer *p, uint64_t rate, uint32_t burst, uint64_t now);

/* A priority send: it goes down at once and never uses tokens. */
void pacer_note_priority(struct pacer *p);

/* A new other send of `bytes` at time `now`. Returns 1 when it may go down
 * now (it then uses tokens), 0 when it must wait at the end of the queue.
 * Order is kept: while anything waits, a new send waits too. A send goes
 * down whenever some tokens are left, even a larger one (the debt is paid
 * by waiting before the next). */
int pacer_offer(struct pacer *p, uint32_t bytes, uint64_t now);

/* The head of the queue holds `bytes`. Returns 1 when it may go down at
 * time `now` (it then leaves the queue and uses tokens), 0 otherwise. */
int pacer_take(struct pacer *p, uint32_t bytes, uint64_t now);

/* Time (100 ns units) from `now` until the queue's head may go down: 0 when
 * it may now or nothing waits. The caller arms its timer with it. */
uint64_t pacer_wait(struct pacer *p, uint64_t now);

/* `count` queued sends of `bytes` in all left the queue without going
 * down (pause, cancel). */
void pacer_drop_queued(struct pacer *p, uint32_t count, uint64_t bytes);

/* The pump: sends the waiting entries that are due, outside the caller's
 * lock, and lets the caller arm its timer for the rest. One pump runs at a
 * time: a call while one runs (the timer on another CPU, or a send the
 * miniport completes inline, which calls the pump again from inside that
 * send) returns at once and the running pump goes on, so the stack never
 * grows with the queue. */
#define PACER_PUMP_BATCH 16u

/* One entry taken off the caller's queue: its send and its flags. */
struct pacer_taken {
    void *item;
    uint32_t flags;
};

struct pacer_pump_ops {
    void (*lock)(void *ctx);
    void (*unlock)(void *ctx);
    /* Under the lock: moves up to `max` due entries, in order, into `out`
     * and returns their number (0 when nothing is due). */
    uint32_t (*take)(void *ctx, struct pacer_taken *out, uint32_t max);
    /* Under the lock: nothing is due now; arm the timer for what waits. */
    void (*idle)(void *ctx);
    /* Outside the lock: one entry goes down. */
    void (*send)(void *ctx, const struct pacer_taken *t);
};

/* Runs the pump unless one runs. `pumping` is the caller's flag, read and
 * written only under its lock. */
void pacer_pump(uint32_t *pumping, const struct pacer_pump_ops *ops, void *ctx);

/* A send of many packets in one NBL (tcpip hands a TCP burst down as one
 * NBL of up to hundreds of packets, dasio#1) is split into child sends of
 * one packet each, paced one by one. The parent completes upward when its
 * last child has completed, with the first failure among them. Called
 * under the caller's lock. */
struct pacer_split {
    uint32_t pending;
    int32_t status;
};

/* A parent split into `children` (at least 1). */
void pacer_split_init(struct pacer_split *s, uint32_t children);

/* One child completed with `status` (0 = success). Returns 1 when it was
 * the last: the parent completes now with `s->status`. */
int pacer_split_done(struct pacer_split *s, int32_t status);

#endif
