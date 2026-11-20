/*
 * dasio kernel RX (dasio#27, B23): the NDIS glue of the RX rings
 * (rxring.c). The decisions, the ring's producer and the chain work are
 * portable C in ../core/rx_ring.c; this side owns the memory, the mapping
 * into the dasio process, the IOCTLs and the hook in the receive path.
 */
#ifndef RXRING_DRIVER_H
#define RXRING_DRIVER_H

#include "../core/rx_ring.h"

// Rings attached to one adapter at most: FOH runs two dasio instances on
// one NIC, and two may receive one group.
#define RX_RINGS_PER_FILTER         4u

typedef struct _RX_RING RX_RING, *PRX_RING;

// One filter's rings (MS_FILTER.RxRing). Rings, Count, Ports and every
// attached ring's sink (table, counters, producer) are written under Lock,
// the RX ring lock; the receive path takes it only for a frame on a port a
// ring names, never the filter's Lock. Lock order: FilterListLock, then
// this Lock.
typedef struct _RX_FILTER_RINGS
{
    // The lock alone on its cache lines, whatever the block's alignment: a
    // line's worth of bytes on each side, so the per-frame lock writes do
    // not bounce what every RSS CPU reads with no lock (Count, Rings, the
    // port bits), nor the filter's fields before it.
    UCHAR                           LockLineBefore[64];
    FILTER_LOCK                     Lock;
    UCHAR                           LockLineAfter[64];
    // Rings attached. The receive path reads it once without the lock: 0
    // (the switch off, or no ring open) means nothing more is done.
    volatile LONG                   Count;
    PRX_RING                        Rings[RX_RINGS_PER_FILTER];
    // Every attached ring's table ports, read without the lock: a frame on
    // another port is never offered, and with none (count 0, every table
    // empty) the receive path does nothing.
    struct pacer_rx_ports           Ports;
    // A whole received frame, when its NET_BUFFER is not contiguous
    // (under Lock).
    UCHAR                           Scratch[PACER_RX_MAX_FRAME];
} RX_FILTER_RINGS, *PRX_FILTER_RINGS;

struct _MS_FILTER;

// <service>\Parameters\RxRingEnable (REG_DWORD, missing = 0), read by
// filter.c with the service's other Parameters when the driver loads. 0:
// IOCTL_DASIO_RX_OPEN fails with STATUS_NOT_SUPPORTED, so no ring ever
// exists and the receive path reads one count.
extern ULONG RxRingEnable;

// Attach: the filter's rings, none yet. Detach: none may be left
// (rxRingDetachAll first), the lock is freed.
VOID rxRingFilterInit(_Inout_ struct _MS_FILTER *Filter);
_IRQL_requires_(PASSIVE_LEVEL)
VOID rxRingFilterTeardown(_Inout_ struct _MS_FILTER *Filter);

// Pause or detach: every ring of the filter is detached (its status shows
// Attached 0); each stays with its handle until closed.
_IRQL_requires_(PASSIVE_LEVEL)
VOID rxRingDetachAll(_Inout_ struct _MS_FILTER *Filter);

// FilterReceiveNetBufferLists, before anything else: a frame of our flows
// goes into each matching ring. Returns TRUE when the call is done with
// the chain (every NBL was stolen and returned, or a chain that cannot
// pend went up from here, in runs); FALSE: *NetBufferLists (*Count of
// them, the stolen ones taken out, the chain untouched when none was) go
// on as before.
BOOLEAN rxRingReceive(_In_ struct _MS_FILTER *Filter, _Inout_ PNET_BUFFER_LIST *NetBufferLists,
                      _Inout_ PULONG Count, _In_ NDIS_PORT_NUMBER PortNumber, _In_ ULONG ReceiveFlags);

// The IOCTLs (device.c), at PASSIVE_LEVEL in the caller's context. *Info
// is the reply's length.
_IRQL_requires_(PASSIVE_LEVEL)
NTSTATUS rxRingOpen(_In_ PIRP Irp, _In_ PIO_STACK_LOCATION IrpSp, _Out_ PULONG Info);
_IRQL_requires_(PASSIVE_LEVEL)
NTSTATUS rxRingMatch(_In_ PIRP Irp, _In_ PIO_STACK_LOCATION IrpSp);
_IRQL_requires_(PASSIVE_LEVEL)
NTSTATUS rxRingClose(_In_ PIO_STACK_LOCATION IrpSp);
_IRQL_requires_(PASSIVE_LEVEL)
NTSTATUS rxRingStatus(_In_ PIRP Irp, _In_ PIO_STACK_LOCATION IrpSp, _Out_ PULONG Info);

// IRP_MJ_CLEANUP: the handle's last close (or its process's exit) closes
// its ring. IRP_MJ_CLOSE calls it again, for a ring an RX_OPEN still in
// flight (an overlapped handle) attached after the cleanup.
_IRQL_requires_(PASSIVE_LEVEL)
VOID rxRingCleanup(_In_ PFILE_OBJECT FileObject);

#endif
