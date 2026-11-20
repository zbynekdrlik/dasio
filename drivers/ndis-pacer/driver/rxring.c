/*
 * dasio kernel RX (dasio#27, B23): received frames of the dasio process's
 * own flows, copied into rings it maps, before tcpip.
 *
 * One ring per open handle of \\.\dasiopacer, on one adapter. Its memory is
 * non-paged, mapped into the opening process in the IOCTL's own context and
 * unmapped there again (RX_CLOSE, or IRP_MJ_CLEANUP when the handle's last
 * close or the process's exit comes; IRP_MJ_CLOSE catches a ring an
 * RX_OPEN on an overlapped handle attached after that cleanup). A pause or
 * detach of the filter detaches its rings; the handle keeps its ring until
 * closed (status Attached 0, the ring's detached word 1: dasio reopens). The decisions and the producer are
 * ../core/rx_ring.c; this file is NDIS and IOCTL glue only.
 *
 * Locks: FilterListLock guards RxRingList (every open ring), each ring's
 * Filter and the filter list; a filter's RxRing.Lock guards its attached
 * rings, their sinks and its Scratch. Order: FilterListLock, then the RX
 * ring lock. The receive path takes only the RX ring lock.
 */

#include "precomp.h"

#define __FILENUMBER    'GNRX'
// Pool tag of a ring's memory (poolmon: DxRg).
#define RX_RING_TAG     'gRxD'

C_ASSERT(DASIO_RX_MAX_MATCHES == PACER_RX_MAX_MATCHES);
C_ASSERT(sizeof(DASIO_RX_STATUS) == 11 * sizeof(ULONG64));

struct _RX_RING
{
    LIST_ENTRY                      Link;           // RxRingList
    PFILE_OBJECT                    FileObject;
    // NULL once detached (written under FilterListLock and the filter's
    // RX ring lock).
    PMS_FILTER                      Filter;
    ULONG                           IfIndex;
    ULONG                           Flags;
    ULONG                           Bytes;
    PVOID                           Memory;
    PMDL                            Mdl;
    PVOID                           UserVa;         // in Process
    PEPROCESS                       Process;        // referenced
    PKEVENT                         Event;          // referenced, or NULL
    struct pacer_rx_sink            Sink;
};

ULONG RxRingEnable = 0;

// Every open ring, attached or not (under FilterListLock).
static LIST_ENTRY RxRingList = { &RxRingList, &RxRingList };


// Under the filter's RX ring lock, as the ring leaves it: its header's
// detached word and its reader's event tell the reader no frame comes any
// more (it closes and opens a new ring).
static VOID
rxSignalDetachedLocked(
    _In_ PRX_RING Ring
    )
{
    pacer_rx_ring_detach(&Ring->Sink.ring);
    if (Ring->Event != NULL)
    {
        KeSetEvent(Ring->Event, IO_NO_INCREMENT, FALSE);
    }
}

_Use_decl_annotations_
VOID
rxRingFilterInit(
    struct _MS_FILTER *Filter
    )
{
    ULONG i;
    FILTER_INIT_LOCK(&Filter->RxRing.Lock);
    Filter->RxRing.Count = 0;
    for (i = 0; i < RX_RINGS_PER_FILTER; i++)
    {
        Filter->RxRing.Rings[i] = NULL;
    }
}

_Use_decl_annotations_
VOID
rxRingDetachAll(
    struct _MS_FILTER *Filter
    )
{
    BOOLEAN bFalse = FALSE;
    BOOLEAN bTrue = TRUE;
    ULONG i;

    FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
    FILTER_ACQUIRE_LOCK(&Filter->RxRing.Lock, bTrue);
    for (i = 0; i < RX_RINGS_PER_FILTER; i++)
    {
        if (Filter->RxRing.Rings[i] != NULL)
        {
            rxSignalDetachedLocked(Filter->RxRing.Rings[i]);
            Filter->RxRing.Rings[i]->Filter = NULL;
            Filter->RxRing.Rings[i] = NULL;
        }
    }
    Filter->RxRing.Count = 0;
    FILTER_RELEASE_LOCK(&Filter->RxRing.Lock, bTrue);
    FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
}

_Use_decl_annotations_
VOID
rxRingFilterTeardown(
    struct _MS_FILTER *Filter
    )
{
    rxRingDetachAll(Filter);
    FILTER_FREE_LOCK(&Filter->RxRing.Lock);
}


// ---- the receive path ------------------------------------------------

// Under the RX ring lock: the attached rings' sinks, and whether any steals.
static ULONG
rxSinksLocked(
    _In_ PRX_FILTER_RINGS Rings,
    _Out_writes_(RX_RINGS_PER_FILTER) struct pacer_rx_sink **Sinks,
    _Out_ PBOOLEAN Steal
    )
{
    ULONG Count = 0;
    ULONG i;
    *Steal = FALSE;
    for (i = 0; i < RX_RINGS_PER_FILTER; i++)
    {
        if (Rings->Rings[i] != NULL)
        {
            Sinks[Count++] = &Rings->Rings[i]->Sink;
            if (Rings->Rings[i]->Sink.steal)
            {
                *Steal = TRUE;
            }
        }
    }
    return Count;
}

// Under the RX ring lock: the readers that asked for a wake get their event,
// once per sleep (the ring's wake word, cleared by the producer).
static VOID
rxWakeLocked(
    _In_ PRX_FILTER_RINGS Rings
    )
{
    ULONG i;
    for (i = 0; i < RX_RINGS_PER_FILTER; i++)
    {
        PRX_RING Ring = Rings->Rings[i];
        if (Ring != NULL && Ring->Sink.wake)
        {
            Ring->Sink.wake = 0;
            if (Ring->Event != NULL)
            {
                KeSetEvent(Ring->Event, IO_NO_INCREMENT, FALSE);
            }
        }
    }
}

// Under the RX ring lock: one received NBL (one Ethernet frame) offered to
// the sinks. Its headers decide (pacer_rx_parse, then the match tables);
// only a frame some ring wants is read whole. Returns TRUE when it must not
// go up (stolen).
static BOOLEAN
rxOfferLocked(
    _In_ PMS_FILTER Filter,
    _In_reads_(Count) struct pacer_rx_sink **Sinks,
    _In_ ULONG Count,
    _In_ PNET_BUFFER_LIST Nbl,
    _In_ ULONG64 Now
    )
{
    UCHAR Storage[PACER_HEADER_BYTES];
    struct pacer_rx_packet Packet;
    PNET_BUFFER Nb = NET_BUFFER_LIST_FIRST_NB(Nbl);
    PUCHAR Header;
    PUCHAR Frame = NULL;
    ULONG Length;
    ULONG Avail;

    // A received Ethernet frame is one NET_BUFFER; anything else goes up.
    if (Nb == NULL || NET_BUFFER_NEXT_NB(Nb) != NULL)
    {
        return FALSE;
    }
    Length = NET_BUFFER_DATA_LENGTH(Nb);
    Avail = Length < PACER_HEADER_BYTES ? Length : PACER_HEADER_BYTES;
    if (Avail == 0)
    {
        return FALSE;
    }
    Header = (PUCHAR)NdisGetDataBuffer(Nb, Avail, Storage, 1, 0);
    if (Header == NULL || !pacer_rx_parse(Header, Avail, Length, &Packet) ||
        !pacer_rx_wanted(Sinks, Count, &Packet))
    {
        return FALSE;
    }
    if (Length <= PACER_RX_MAX_FRAME)
    {
        // NULL (unreadable) is counted with the too big ones and goes up.
        Frame = (PUCHAR)NdisGetDataBuffer(Nb, Length, Filter->RxRing.Scratch, 1, 0);
    }
    return pacer_rx_deliver(Sinks, Count, &Packet, Frame, Length, Now) != 0 ? TRUE : FALSE;
}

_Use_decl_annotations_
BOOLEAN
rxRingReceive(
    struct _MS_FILTER *Filter,
    PNET_BUFFER_LIST *NetBufferLists,
    PULONG NumberOfNetBufferLists,
    NDIS_PORT_NUMBER PortNumber,
    ULONG ReceiveFlags
    )
{
    PRX_FILTER_RINGS Rings = &Filter->RxRing;
    struct pacer_rx_sink *Sinks[RX_RINGS_PER_FILTER];
    BOOLEAN DispatchLevel;
    BOOLEAN Steal = FALSE;
    ULONG Count;
    ULONG64 Qpc;
    ULONG64 Now;
    PNET_BUFFER_LIST Nbl;
    PNET_BUFFER_LIST Next;

    // No ring (the switch off, or none open): this read and nothing else.
    if (Rings->Count == 0 || PortNumber != NDIS_DEFAULT_PORT_NUMBER)
    {
        return FALSE;
    }
    DispatchLevel = NDIS_TEST_RECEIVE_AT_DISPATCH_LEVEL(ReceiveFlags);
    Now = KeQueryInterruptTimePrecise(&Qpc);

    FILTER_ACQUIRE_LOCK(&Rings->Lock, DispatchLevel);
    Count = rxSinksLocked(Rings, Sinks, &Steal);
    if (!Steal || NDIS_TEST_RECEIVE_CAN_PEND(ReceiveFlags))
    {
        // One pass under the lock. Copy mode only: the chain goes on
        // untouched. Stealing and the NBLs may pend: the stolen ones are
        // taken out (order kept) and go back to the miniport now.
        PNET_BUFFER_LIST Keep = NULL;
        PNET_BUFFER_LIST *KeepTail = &Keep;
        PNET_BUFFER_LIST Stolen = NULL;
        PNET_BUFFER_LIST *StolenTail = &Stolen;
        ULONG KeepCount = 0;

        for (Nbl = *NetBufferLists; Nbl != NULL; Nbl = Next)
        {
            BOOLEAN Take;
            Next = NET_BUFFER_LIST_NEXT_NBL(Nbl);
            Take = (Count != 0 && rxOfferLocked(Filter, Sinks, Count, Nbl, Now)) ? TRUE : FALSE;
            if (!Steal)
            {
                continue;
            }
            NET_BUFFER_LIST_NEXT_NBL(Nbl) = NULL;
            if (Take)
            {
                *StolenTail = Nbl;
                StolenTail = &NET_BUFFER_LIST_NEXT_NBL(Nbl);
            }
            else
            {
                *KeepTail = Nbl;
                KeepTail = &NET_BUFFER_LIST_NEXT_NBL(Nbl);
                KeepCount++;
            }
        }
        rxWakeLocked(Rings);
        FILTER_RELEASE_LOCK(&Rings->Lock, DispatchLevel);
        if (!Steal)
        {
            return FALSE;
        }
        if (Stolen != NULL)
        {
            NdisFReturnNetBufferLists(Filter->FilterHandle, Stolen,
                                      DispatchLevel ? NDIS_RETURN_FLAGS_DISPATCH_LEVEL : 0);
        }
        *NetBufferLists = Keep;
        *NumberOfNetBufferLists = KeepCount;
        return Keep == NULL ? TRUE : FALSE;
    }
    FILTER_RELEASE_LOCK(&Rings->Lock, DispatchLevel);

    // Stealing, and the NBLs cannot pend (the miniport needs them back when
    // this returns): NBL by NBL, each one that stays goes up alone, its own
    // link cut for the call and restored right after, so the miniport gets
    // its chain back as it was; a stolen one simply is not indicated.
    {
        ULONG Indicated = 0;
        for (Nbl = *NetBufferLists; Nbl != NULL; Nbl = Next)
        {
            BOOLEAN Take;
            Next = NET_BUFFER_LIST_NEXT_NBL(Nbl);
            FILTER_ACQUIRE_LOCK(&Rings->Lock, DispatchLevel);
            Count = rxSinksLocked(Rings, Sinks, &Steal);
            Take = (Count != 0 && rxOfferLocked(Filter, Sinks, Count, Nbl, Now)) ? TRUE : FALSE;
            rxWakeLocked(Rings);
            FILTER_RELEASE_LOCK(&Rings->Lock, DispatchLevel);
            if (!Take)
            {
                NET_BUFFER_LIST_NEXT_NBL(Nbl) = NULL;
                NdisFIndicateReceiveNetBufferLists(Filter->FilterHandle, Nbl, PortNumber, 1, ReceiveFlags);
                NET_BUFFER_LIST_NEXT_NBL(Nbl) = Next;
                Indicated++;
            }
        }
        // rxd's count of NBLs that went up at once.
        FILTER_ACQUIRE_LOCK(&Filter->Lock, DispatchLevel);
        Filter->RxdInline += Indicated;
        FILTER_RELEASE_LOCK(&Filter->Lock, DispatchLevel);
    }
    return TRUE;
}


// ---- the IOCTLs --------------------------------------------------------

// Under FilterListLock: the ring of a handle.
static PRX_RING
rxFindLocked(
    _In_ PFILE_OBJECT FileObject
    )
{
    PLIST_ENTRY Link;
    for (Link = RxRingList.Flink; Link != &RxRingList; Link = Link->Flink)
    {
        PRX_RING Ring = CONTAINING_RECORD(Link, RX_RING, Link);
        if (Ring->FileObject == FileObject)
        {
            return Ring;
        }
    }
    return NULL;
}

// Under FilterListLock: the bound adapter with this IfIndex.
static PMS_FILTER
rxFilterLocked(
    _In_ ULONG IfIndex
    )
{
    PLIST_ENTRY Link;
    for (Link = FilterModuleList.Flink; Link != &FilterModuleList; Link = Link->Flink)
    {
        PMS_FILTER Filter = CONTAINING_RECORD(Link, MS_FILTER, FilterModuleLink);
        if (Filter->MiniportIfIndex == IfIndex)
        {
            return Filter;
        }
    }
    return NULL;
}

// Under FilterListLock: the ring off its filter (under the filter's RX ring
// lock): no receive reaches it after this.
static VOID
rxDetachLocked(
    _In_ PRX_RING Ring
    )
{
    PMS_FILTER Filter = Ring->Filter;
    BOOLEAN bTrue = TRUE;
    ULONG i;
    if (Filter == NULL)
    {
        return;
    }
    FILTER_ACQUIRE_LOCK(&Filter->RxRing.Lock, bTrue);
    for (i = 0; i < RX_RINGS_PER_FILTER; i++)
    {
        if (Filter->RxRing.Rings[i] == Ring)
        {
            Filter->RxRing.Rings[i] = NULL;
            Filter->RxRing.Count--;
        }
    }
    rxSignalDetachedLocked(Ring);
    Ring->Filter = NULL;
    FILTER_RELEASE_LOCK(&Filter->RxRing.Lock, bTrue);
}

// At PASSIVE_LEVEL, the ring detached and off RxRingList. Unmap: in the
// process that mapped it, the mapping goes, then the memory. Without it
// (the handle's last close came from another process, which cannot reach
// the owner's mapping) the memory stays allocated where the owner may still
// see it: never freed under a live mapping.
static VOID
rxRingFree(
    _In_ PRX_RING Ring,
    _In_ BOOLEAN Unmap
    )
{
    if (Ring->UserVa != NULL && Unmap)
    {
        MmUnmapLockedPages(Ring->UserVa, Ring->Mdl);
        Ring->UserVa = NULL;
    }
    if (Ring->UserVa == NULL)
    {
        if (Ring->Mdl != NULL)
        {
            IoFreeMdl(Ring->Mdl);
        }
        if (Ring->Memory != NULL)
        {
            ExFreePoolWithTag(Ring->Memory, RX_RING_TAG);
        }
    }
    else
    {
        DEBUGP(DL_WARN, "rxRingFree: ring of %u bytes left mapped (closed from another process)\n", Ring->Bytes);
    }
    if (Ring->Event != NULL)
    {
        ObDereferenceObject(Ring->Event);
    }
    if (Ring->Process != NULL)
    {
        ObDereferenceObject(Ring->Process);
    }
    ExFreePoolWithTag(Ring, FILTER_ALLOC_TAG);
}

// Under FilterListLock: attaches a new ring to the adapter `IfIndex` and
// gives it to its handle.
static NTSTATUS
rxAttachLocked(
    _In_ PRX_RING Ring
    )
{
    PMS_FILTER Filter;
    BOOLEAN bTrue = TRUE;
    NTSTATUS Status = STATUS_INSUFFICIENT_RESOURCES;
    ULONG i;

    if (rxFindLocked(Ring->FileObject) != NULL)
    {
        return STATUS_INVALID_DEVICE_STATE;
    }
    Filter = rxFilterLocked(Ring->IfIndex);
    if (Filter == NULL)
    {
        return STATUS_NOT_FOUND;
    }
    // A pause detaches every ring after setting Pausing, under this list
    // lock: a ring attached now is detached by it, one attached after it is
    // refused here.
    if (Filter->State != FilterRunning)
    {
        return STATUS_DEVICE_NOT_READY;
    }
    FILTER_ACQUIRE_LOCK(&Filter->RxRing.Lock, bTrue);
    for (i = 0; i < RX_RINGS_PER_FILTER; i++)
    {
        if (Filter->RxRing.Rings[i] == NULL)
        {
            Filter->RxRing.Rings[i] = Ring;
            Filter->RxRing.Count++;
            Ring->Filter = Filter;
            Status = STATUS_SUCCESS;
            break;
        }
    }
    FILTER_RELEASE_LOCK(&Filter->RxRing.Lock, bTrue);
    if (NT_SUCCESS(Status))
    {
        InsertTailList(&RxRingList, &Ring->Link);
    }
    return Status;
}

_Use_decl_annotations_
NTSTATUS
rxRingOpen(
    PIRP Irp,
    PIO_STACK_LOCATION IrpSp,
    PULONG Info
    )
{
    PUCHAR Buffer = (PUCHAR)Irp->AssociatedIrp.SystemBuffer;
    ULONG InLength = IrpSp->Parameters.DeviceIoControl.InputBufferLength;
    ULONG OutLength = IrpSp->Parameters.DeviceIoControl.OutputBufferLength;
    DASIO_RX_OPEN Open;
    PRX_RING Ring;
    ULONG Bytes;
    BOOLEAN bFalse = FALSE;
    NTSTATUS Status;

    *Info = 0;
    if (!RxRingEnable)
    {
        return STATUS_NOT_SUPPORTED;
    }
    // The ring is mapped into the calling process: a user-mode caller only.
    if (Irp->RequestorMode != UserMode)
    {
        return STATUS_INVALID_DEVICE_REQUEST;
    }
    if (InLength < sizeof(DASIO_RX_OPEN) || OutLength < sizeof(DASIO_RX_OPENED))
    {
        return STATUS_INVALID_PARAMETER;
    }
    RtlCopyMemory(&Open, Buffer, sizeof(Open));
    Bytes = pacer_rx_ring_bytes(Open.SlotCount);
    if (Bytes == 0 || (Open.Flags & ~DASIO_RX_FLAGS) != 0 || Open.Reserved != 0)
    {
        return STATUS_INVALID_PARAMETER;
    }
    // No adapter with that IfIndex: refused before any memory is taken
    // (checked again when attaching).
    FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
    Status = rxFilterLocked(Open.IfIndex) != NULL ? STATUS_SUCCESS : STATUS_NOT_FOUND;
    FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
    if (!NT_SUCCESS(Status))
    {
        return Status;
    }

    Ring = (PRX_RING)ExAllocatePool2(POOL_FLAG_NON_PAGED, sizeof(RX_RING), FILTER_ALLOC_TAG);
    if (Ring == NULL)
    {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    RtlZeroMemory(Ring, sizeof(RX_RING));
    Ring->FileObject = IrpSp->FileObject;
    Ring->IfIndex = Open.IfIndex;
    Ring->Flags = Open.Flags;
    Ring->Bytes = Bytes;
    Ring->Sink.steal = (Open.Flags & DASIO_RX_STEAL) != 0;
    Ring->Process = IoGetCurrentProcess();
    ObReferenceObject(Ring->Process);

    Status = STATUS_INSUFFICIENT_RESOURCES;
    do
    {
        // Whole pages (a large pool block is page aligned, and the size is
        // a multiple of a page): the mapping shows the caller nothing else.
        Ring->Memory = ExAllocatePool2(POOL_FLAG_NON_PAGED, Bytes, RX_RING_TAG);
        if (Ring->Memory == NULL || ((ULONG_PTR)Ring->Memory & (PAGE_SIZE - 1)) != 0 ||
            (Bytes & (PAGE_SIZE - 1)) != 0)
        {
            break;
        }
        (void)pacer_rx_ring_init(&Ring->Sink.ring, (PUCHAR)Ring->Memory, Open.SlotCount);
        Ring->Mdl = IoAllocateMdl(Ring->Memory, Bytes, FALSE, FALSE, NULL);
        if (Ring->Mdl == NULL)
        {
            break;
        }
        MmBuildMdlForNonPagedPool(Ring->Mdl);
        if (Open.Event != 0)
        {
            Status = ObReferenceObjectByHandle((HANDLE)(ULONG_PTR)Open.Event, EVENT_MODIFY_STATE,
                                               *ExEventObjectType, UserMode, (PVOID *)&Ring->Event, NULL);
            if (!NT_SUCCESS(Status))
            {
                Ring->Event = NULL;
                Status = STATUS_INVALID_PARAMETER;
                break;
            }
            Status = STATUS_INSUFFICIENT_RESOURCES;
        }
        __try
        {
            Ring->UserVa = MmMapLockedPagesSpecifyCache(Ring->Mdl, UserMode, MmCached, NULL, FALSE,
                                                        NormalPagePriority | MdlMappingNoExecute);
        }
        __except (EXCEPTION_EXECUTE_HANDLER)
        {
            Ring->UserVa = NULL;
        }
        if (Ring->UserVa == NULL)
        {
            break;
        }
        FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
        Status = rxAttachLocked(Ring);
        FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
    } while (bFalse);

    if (!NT_SUCCESS(Status))
    {
        rxRingFree(Ring, TRUE);
        return Status;
    }
    {
        PDASIO_RX_OPENED Out = (PDASIO_RX_OPENED)Buffer;
        Out->Address = (ULONG64)(ULONG_PTR)Ring->UserVa;
        Out->Bytes = Bytes;
        *Info = sizeof(DASIO_RX_OPENED);
    }
    DEBUGP(DL_INFO, "rxRingOpen: IfIndex %u, %u slots, flags %x\n", Open.IfIndex, Open.SlotCount, Open.Flags);
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS
rxRingMatch(
    PIRP Irp,
    PIO_STACK_LOCATION IrpSp
    )
{
    PDASIO_RX_MATCH Input = (PDASIO_RX_MATCH)Irp->AssociatedIrp.SystemBuffer;
    ULONG InLength = IrpSp->Parameters.DeviceIoControl.InputBufferLength;
    struct pacer_rx_match_entry Table[PACER_RX_MAX_MATCHES];
    PRX_RING Ring;
    BOOLEAN bFalse = FALSE;
    BOOLEAN bTrue = TRUE;
    ULONG Count;
    ULONG i;

    if (InLength < (ULONG)FIELD_OFFSET(DASIO_RX_MATCH, Entries))
    {
        return STATUS_INVALID_PARAMETER;
    }
    Count = Input->Count;
    if (Count > DASIO_RX_MAX_MATCHES || Input->Reserved != 0 ||
        InLength < (ULONG)FIELD_OFFSET(DASIO_RX_MATCH, Entries) + Count * (ULONG)sizeof(DASIO_RX_MATCH_ENTRY))
    {
        return STATUS_INVALID_PARAMETER;
    }
    for (i = 0; i < Count; i++)
    {
        if (Input->Entries[i].Reserved != 0)
        {
            return STATUS_INVALID_PARAMETER;
        }
        RtlCopyMemory(Table[i].addr, Input->Entries[i].Address, 4);
        Table[i].port = Input->Entries[i].Port;
    }
    if (!pacer_rx_match_valid(Table, Count))
    {
        return STATUS_INVALID_PARAMETER;
    }

    FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
    Ring = rxFindLocked(IrpSp->FileObject);
    if (Ring != NULL)
    {
        // The whole table at once, under the RX ring lock while attached.
        PMS_FILTER Filter = Ring->Filter;
        if (Filter != NULL)
        {
            FILTER_ACQUIRE_LOCK(&Filter->RxRing.Lock, bTrue);
        }
        (void)pacer_rx_set_match(&Ring->Sink, Table, Count);
        if (Filter != NULL)
        {
            FILTER_RELEASE_LOCK(&Filter->RxRing.Lock, bTrue);
        }
    }
    FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
    return Ring != NULL ? STATUS_SUCCESS : STATUS_INVALID_DEVICE_STATE;
}

_Use_decl_annotations_
NTSTATUS
rxRingClose(
    PIO_STACK_LOCATION IrpSp
    )
{
    PRX_RING Ring;
    BOOLEAN bFalse = FALSE;
    NTSTATUS Status = STATUS_SUCCESS;

    FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
    Ring = rxFindLocked(IrpSp->FileObject);
    if (Ring == NULL)
    {
        Status = STATUS_INVALID_DEVICE_STATE;
    }
    else if (Ring->Process != IoGetCurrentProcess())
    {
        // Its mapping is in the process that opened it: only that one.
        Status = STATUS_ACCESS_DENIED;
    }
    else
    {
        rxDetachLocked(Ring);
        RemoveEntryList(&Ring->Link);
    }
    FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
    if (NT_SUCCESS(Status))
    {
        rxRingFree(Ring, TRUE);
    }
    return Status;
}

_Use_decl_annotations_
VOID
rxRingCleanup(
    PFILE_OBJECT FileObject
    )
{
    PRX_RING Ring;
    BOOLEAN bFalse = FALSE;

    FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
    Ring = rxFindLocked(FileObject);
    if (Ring != NULL)
    {
        rxDetachLocked(Ring);
        RemoveEntryList(&Ring->Link);
    }
    FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
    if (Ring != NULL)
    {
        rxRingFree(Ring, Ring->Process == IoGetCurrentProcess());
    }
}

_Use_decl_annotations_
NTSTATUS
rxRingStatus(
    PIRP Irp,
    PIO_STACK_LOCATION IrpSp,
    PULONG Info
    )
{
    PUCHAR Output = (PUCHAR)Irp->AssociatedIrp.SystemBuffer;
    ULONG OutLength = IrpSp->Parameters.DeviceIoControl.OutputBufferLength;
    NTSTATUS Status = STATUS_SUCCESS;
    PLIST_ENTRY Link;
    BOOLEAN bFalse = FALSE;
    BOOLEAN bTrue = TRUE;
    ULONG Length = 0;

    FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
    for (Link = RxRingList.Flink; Link != &RxRingList; Link = Link->Flink)
    {
        PRX_RING Ring = CONTAINING_RECORD(Link, RX_RING, Link);
        PMS_FILTER Filter = Ring->Filter;
        PDASIO_RX_STATUS Out;
        if (Length + (ULONG)sizeof(DASIO_RX_STATUS) > OutLength)
        {
            Status = STATUS_BUFFER_TOO_SMALL;
            break;
        }
        Out = (PDASIO_RX_STATUS)(Output + Length);
        if (Filter != NULL)
        {
            FILTER_ACQUIRE_LOCK(&Filter->RxRing.Lock, bTrue);
        }
        Out->Version = DASIO_RX_STATUS_VERSION;
        Out->IfIndex = Ring->IfIndex;
        Out->Attached = Filter != NULL ? 1 : 0;
        Out->Flags = Ring->Flags;
        Out->SlotCount = Ring->Sink.ring.slots;
        Out->Matches = Ring->Sink.matches;
        Out->Matched = Ring->Sink.matched;
        Out->Delivered = Ring->Sink.delivered;
        Out->Full = Ring->Sink.full;
        Out->Stolen = Ring->Sink.stolen;
        Out->TooBig = Ring->Sink.too_big;
        if (Filter != NULL)
        {
            FILTER_RELEASE_LOCK(&Filter->RxRing.Lock, bTrue);
        }
        Length += (ULONG)sizeof(DASIO_RX_STATUS);
    }
    FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
    *Info = Status == STATUS_BUFFER_TOO_SMALL ? 0 : Length;
    return Status;
}
