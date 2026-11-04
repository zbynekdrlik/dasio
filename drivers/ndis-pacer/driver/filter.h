/*++

Copyright (c) Microsoft Corporation

Module Name:

    Filter.h

Abstract:

    This module contains all prototypes and macros for filter code.

Notes:

--*/
#ifndef _FILT_H
#define _FILT_H

#pragma warning(disable:28930) // Unused assignment of pointer, by design in samples
#pragma warning(disable:28931) // Unused assignment of variable, by design in samples

// TODO: Customize these to hint at your component for memory leak tracking.
// These should be treated like a pooltag.
#define FILTER_REQUEST_ID          'RTLF'
#define FILTER_ALLOC_TAG           'tliF'
#define FILTER_TAG                 'dnTF'

// TODO: Specify which version of the NDIS contract you will use here.
// In many cases, 6.0 is the best choice.  You only need to select a later
// version if you need a feature that is not available in 6.0.
//
// Legal values include:
//    6.0  Available starting with Windows Vista RTM
//    6.1  Available starting with Windows Vista SP1 / Windows Server 2008
//    6.20 Available starting with Windows 7 / Windows Server 2008 R2
//    6.30 Available starting with Windows 8 / Windows Server "8"
// Or, just use NDIS_FILTER_MAJOR_VERSION / NDIS_FILTER_MINOR_VERSION
// to pick up whatever version is defined by your build system
// (for example, "-DNDIS630").
#define FILTER_MAJOR_NDIS_VERSION   NDIS_FILTER_MAJOR_VERSION
#define FILTER_MINOR_NDIS_VERSION   NDIS_FILTER_MINOR_VERSION


//
// Global variables
//
extern NDIS_HANDLE         FilterDriverHandle; // NDIS handle for filter driver
extern NDIS_HANDLE         FilterDriverObject;
extern NDIS_HANDLE         NdisFilterDeviceHandle;
extern PDEVICE_OBJECT      NdisDeviceObject;

extern FILTER_LOCK         FilterListLock;
extern LIST_ENTRY          FilterModuleList;




#if NDISLWF
#define FILTER_FRIENDLY_NAME        L"dasio NDIS send pacer"
// TODO: Customize this to match the GUID in the INF
#define FILTER_UNIQUE_NAME          L"{7c3f2a51-9d0e-4b8a-a6f1-3e5d2c8b9a40}" //unique name, quid name
// TODO: Customize this to match the service name in the INF
#define FILTER_SERVICE_NAME         L"dasiopacer"
//
// The filter needs to handle IOCTLs
//
#define LINKNAME_STRING             L"\\DosDevices\\dasiopacer"
#define NTDEVICE_STRING             L"\\Device\\dasiopacer"
#endif


#if NDISLWF1
#define FILTER_FRIENDLY_NAME        L"NDIS Sample LightWeight Filter 1"
#define FILTER_UNIQUE_NAME          L"{5cbf81be-5055-47cd-9055-a76b2b4e3697}" //unique name, quid name
#define FILTER_SERVICE_NAME         L"NDISLWF1"
//
// The filter needs to handle IOCTRLs
//
#define LINKNAME_STRING             L"\\DosDevices\\NDISLWF1"
#define NTDEVICE_STRING             L"\\Device\\NDISLWF1"
#endif

#if NDISMON
#define FILTER_FRIENDLY_NAME        L"NDIS Sample Monitor LightWeight Filter"
#define FILTER_UNIQUE_NAME          L"{5cbf81bf-5055-47cd-9055-a76b2b4e3697}" //unique name, quid name
#define FILTER_SERVICE_NAME         L"NDISMON"
//
// The filter needs to handle IOCTRLs
//
#define LINKNAME_STRING             L"\\DosDevices\\NDISMON"
#define NTDEVICE_STRING             L"\\Device\\NDISMON"
#endif

#if NDISMON1
#define FILTER_FRIENDLY_NAME        L"NDIS Sample Monitor 1 LightWeight Filter"
#define FILTER_UNIQUE_NAME          L"{5cbf81c0-5055-47cd-9055-a76b2b4e3697}" //unique name, quid name
#define FILTER_SERVICE_NAME         L"NDISMON1"
//
// The filter needs to handle IOCTRLs
//
#define LINKNAME_STRING             L"\\DosDevices\\NDISMON1"
#define NTDEVICE_STRING             L"\\Device\\NDISMON1"
#endif


//
// Types and macros to manipulate packet queue
//
typedef struct _QUEUE_ENTRY
{
    struct _QUEUE_ENTRY * Next;
}QUEUE_ENTRY, *PQUEUE_ENTRY;

typedef struct _QUEUE_HEADER
{
    PQUEUE_ENTRY     Head;
    PQUEUE_ENTRY     Tail;
} QUEUE_HEADER, *PQUEUE_HEADER;


#if TRACK_RECEIVES
UINT         filterLogReceiveRefIndex = 0;
ULONG_PTR    filterLogReceiveRef[0x10000];
#endif

#if TRACK_SENDS
UINT         filterLogSendRefIndex = 0;
ULONG_PTR    filterLogSendRef[0x10000];
#endif

#if TRACK_RECEIVES
#define   FILTER_LOG_RCV_REF(_O, _Instance, _NetBufferList, _Ref)    \
    {\
        filterLogReceiveRef[filterLogReceiveRefIndex++] = (ULONG_PTR)(_O); \
        filterLogReceiveRef[filterLogReceiveRefIndex++] = (ULONG_PTR)(_Instance); \
        filterLogReceiveRef[filterLogReceiveRefIndex++] = (ULONG_PTR)(_NetBufferList); \
        filterLogReceiveRef[filterLogReceiveRefIndex++] = (ULONG_PTR)(_Ref); \
        if (filterLogReceiveRefIndex >= (0x10000 - 5))                    \
        {                                                              \
            filterLogReceiveRefIndex = 0;                                 \
        }                                                              \
    }
#else
#define   FILTER_LOG_RCV_REF(_O, _Instance, _NetBufferList, _Ref)
#endif

#if TRACK_SENDS
#define   FILTER_LOG_SEND_REF(_O, _Instance, _NetBufferList, _Ref)    \
    {\
        filterLogSendRef[filterLogSendRefIndex++] = (ULONG_PTR)(_O); \
        filterLogSendRef[filterLogSendRefIndex++] = (ULONG_PTR)(_Instance); \
        filterLogSendRef[filterLogSendRefIndex++] = (ULONG_PTR)(_NetBufferList); \
        filterLogSendRef[filterLogSendRefIndex++] = (ULONG_PTR)(_Ref); \
        if (filterLogSendRefIndex >= (0x10000 - 5))                    \
        {                                                              \
            filterLogSendRefIndex = 0;                                 \
        }                                                              \
    }

#else
#define   FILTER_LOG_SEND_REF(_O, _Instance, _NetBufferList, _Ref)
#endif


//
// DEBUG related macros.
//
#if DBG
#define FILTER_ALLOC_MEM(_NdisHandle, _Size)    \
    filterAuditAllocMem(                        \
            _NdisHandle,                        \
           _Size,                               \
           __FILENUMBER,                        \
           __LINE__);

#define FILTER_FREE_MEM(_pMem)                  \
    filterAuditFreeMem(_pMem);

#else
#define FILTER_ALLOC_MEM(_NdisHandle, _Size)     \
    NdisAllocateMemoryWithTagPriority(_NdisHandle, _Size, FILTER_ALLOC_TAG, LowPoolPriority)

#define FILTER_FREE_MEM(_pMem)    NdisFreeMemory(_pMem, 0, 0)

#endif //DBG

#if DBG_SPIN_LOCK
#define FILTER_INIT_LOCK(_pLock)                          \
    filterAllocateSpinLock(_pLock, __FILENUMBER, __LINE__)

#define FILTER_FREE_LOCK(_pLock)       filterFreeSpinLock(_pLock)


#define FILTER_ACQUIRE_LOCK(_pLock, DispatchLevel)  \
    filterAcquireSpinLock(_pLock, __FILENUMBER, __LINE__, DisaptchLevel)

#define FILTER_RELEASE_LOCK(_pLock, DispatchLevel)      \
    filterReleaseSpinLock(_pLock, __FILENUMBER, __LINE__, DispatchLevel)

#else
#define FILTER_INIT_LOCK(_pLock)      NdisAllocateSpinLock(_pLock)

#define FILTER_FREE_LOCK(_pLock)      NdisFreeSpinLock(_pLock)

#define FILTER_ACQUIRE_LOCK(_pLock, DispatchLevel)              \
    {                                                           \
        if (DispatchLevel)                                      \
        {                                                       \
            NdisDprAcquireSpinLock(_pLock);                     \
        }                                                       \
        else                                                    \
        {                                                       \
            NdisAcquireSpinLock(_pLock);                        \
        }                                                       \
    }

#define FILTER_RELEASE_LOCK(_pLock, DispatchLevel)              \
    {                                                           \
        if (DispatchLevel)                                      \
        {                                                       \
            NdisDprReleaseSpinLock(_pLock);                     \
        }                                                       \
        else                                                    \
        {                                                       \
            NdisReleaseSpinLock(_pLock);                        \
        }                                                       \
    }
#endif //DBG_SPIN_LOCK


#define NET_BUFFER_LIST_LINK_TO_ENTRY(_pNBL)    ((PQUEUE_ENTRY)(NET_BUFFER_LIST_NEXT_NBL(_pNBL)))
#define ENTRY_TO_NET_BUFFER_LIST(_pEnt)         (CONTAINING_RECORD((_pEnt), NET_BUFFER_LIST, Next))

#define InitializeQueueHeader(_QueueHeader)             \
{                                                       \
    (_QueueHeader)->Head = (_QueueHeader)->Tail = NULL; \
}

//
// Macros for queue operations
//
#define IsQueueEmpty(_QueueHeader)      ((_QueueHeader)->Head == NULL)

#define RemoveHeadQueue(_QueueHeader)                   \
    (_QueueHeader)->Head;                               \
    {                                                   \
        PQUEUE_ENTRY pNext;                             \
        ASSERT((_QueueHeader)->Head);                   \
        pNext = (_QueueHeader)->Head->Next;             \
        (_QueueHeader)->Head = pNext;                   \
        if (pNext == NULL)                              \
            (_QueueHeader)->Tail = NULL;                \
    }

#define InsertHeadQueue(_QueueHeader, _QueueEntry)                  \
    {                                                               \
        ((PQUEUE_ENTRY)(_QueueEntry))->Next = (_QueueHeader)->Head; \
        (_QueueHeader)->Head = (PQUEUE_ENTRY)(_QueueEntry);         \
        if ((_QueueHeader)->Tail == NULL)                           \
            (_QueueHeader)->Tail = (PQUEUE_ENTRY)(_QueueEntry);     \
    }

#define InsertTailQueue(_QueueHeader, _QueueEntry)                      \
    {                                                                   \
        ((PQUEUE_ENTRY)(_QueueEntry))->Next = NULL;                     \
        if ((_QueueHeader)->Tail)                                       \
            (_QueueHeader)->Tail->Next = (PQUEUE_ENTRY)(_QueueEntry);   \
        else                                                            \
            (_QueueHeader)->Head = (PQUEUE_ENTRY)(_QueueEntry);         \
        (_QueueHeader)->Tail = (PQUEUE_ENTRY)(_QueueEntry);             \
    }


//
// Enum of filter's states
// Filter can only be in one state at one time
//
typedef enum _FILTER_STATE
{
    FilterStateUnspecified,
    FilterInitialized,
    FilterPausing,
    FilterPaused,
    FilterRunning,
    FilterRestarting,
    FilterDetaching
} FILTER_STATE;


typedef struct _FILTER_REQUEST
{
    NDIS_OID_REQUEST       Request;
    NDIS_EVENT             ReqEvent;
    NDIS_STATUS            Status;
} FILTER_REQUEST, *PFILTER_REQUEST;

//
// Define the filter struct
//
#include "../core/pacer_core.h"

// Kernel test stream and media TX: frames that may be out at once and the
// bytes of one slot. Media TX of two dasio devices on the FOH PC is about
// 150 000 frames a second; with Realtek 10.79 the miniport held all 256
// slots uncompleted at moments (4.11.2026: 874 frames went to the socket
// instead), and it holds frames for up to 20 ms (test stream). 4096 slots
// cover ~27 ms (6 MB of nonpaged memory).
#define KTX_SLOTS_DECL              4096
#define KTX_SLOTS                   4096u
#define KTX_SLOT_BYTES              1536u
// Frames sent in one timer fire at most (a late fire catches up).
#define KTX_MAX_BATCH               128u
// The timer's period (100 ns): 1 ms.
#define KTX_TIMER_PERIOD            10000

// One waiting send: the NBL and the flags it was sent with (default port).
typedef struct _PACER_ENTRY
{
    PNET_BUFFER_LIST    Nbl;
    ULONG               SendFlags;
} PACER_ENTRY, *PPACER_ENTRY;

// Waiting sends at most; with a full ring a send goes down unpaced.
#define PACER_RING_SIZE             4096u

typedef struct _MS_FILTER
{
    LIST_ENTRY                     FilterModuleLink;
    //Reference to this filter
    ULONG                           RefCount;

    NDIS_HANDLE                     FilterHandle;
    NDIS_STRING                     FilterModuleName;
    NDIS_STRING                     MiniportFriendlyName;
    NDIS_STRING                     MiniportName;
    NET_IFINDEX                     MiniportIfIndex;

    NDIS_STATUS                     Status;
    NDIS_EVENT                      Event;
    ULONG                           BackFillSize;
    FILTER_LOCK                     Lock;    // Lock for protection of state and outstanding sends and recvs

    FILTER_STATE                    State;   // Which state the filter is in
    ULONG                           OutstandingSends;
    ULONG                           OutstandingRequest;
    ULONG                           OutstandingRcvs;
    FILTER_LOCK                     SendLock;
    FILTER_LOCK                     RcvLock;
    QUEUE_HEADER                    SendNBLQueue;
    QUEUE_HEADER                    RcvNBLQueue;


    NDIS_STRING                     FilterName;
    ULONG                           CallsRestart;
    BOOLEAN                         TrackReceives;
    BOOLEAN                         TrackSends;
#if DBG
    BOOLEAN                         bIndicating;
#endif

    PNDIS_OID_REQUEST               PendingOidRequest;

    // dasio send pacer (all under Lock): the decisions and the other
    // sends waiting for tokens, in order, in a ring of the filter's own
    // (no field of a waiting NBL is touched: the protocol above owns its
    // Scratch and reserved fields).
    struct pacer                    Pacer;
    struct _PACER_ENTRY            *PaceRing;
    ULONG                           PaceRingHead;
    ULONG                           PaceRingCount;
    // Sends the queue's head when tokens are back (high resolution).
    PEX_TIMER                       PaceTimer;
    BOOLEAN                         PaceTimerArmed;
    // A pump runs: a nested call (a send completed inline) returns at once
    // and the running pump goes on, so the stack never grows per NBL.
    ULONG                           PacePumping;
    // What IOCTL_DASIO_PACER_STATUS reports (under Lock).
    ULONG64                         PaceLinkAtAttach;
    ULONG64                         PaceLinkLast;
    ULONG64                         PaceLinkUpdates;
    ULONG64                         PaceTimerArms;
    ULONG64                         PaceTimerFires;
    ULONG64                         PacePumps;
    ULONG64                         PacePumpSent;
    ULONG64                         PaceOtherNbls;
    ULONG64                         PaceOtherNbs;
    ULONG64                         PaceMaxNbs;
    ULONG64                         PaceMaxNblBytes;
    // Multi-packet NBLs split into child NBLs of one packet (dasio#1):
    // the filter's own pool, and the counts the status reports.
    NDIS_HANDLE                     PaceNblPool;
    ULONG64                         PaceSplits;
    ULONG64                         PaceSplitChildren;
    ULONG64                         PaceSplitFails;
    ULONG64                         PaceChildrenOut;

    // dasio kernel test stream (ktx, dasio#1): frames built in slots of
    // the filter's own and sent straight down from a high resolution
    // timer. Slots, pool and timer are made at attach (no stream, no
    // sends); all fields under Lock.
    UCHAR                           KtxSrcMac[6];
    BOOLEAN                         KtxReady;
    BOOLEAN                         KtxActive;
    PUCHAR                          KtxBuf;          // KTX_SLOTS frames of KTX_SLOT_BYTES
    PMDL                            KtxMdl[KTX_SLOTS_DECL];
    ULONG                           KtxFree[KTX_SLOTS_DECL];
    ULONG                           KtxFreeCount;
    NDIS_HANDLE                     KtxPool;
    PEX_TIMER                       KtxTimer;
    struct pacer_ktx_addr           KtxAddr;
    ULONG                           KtxPayload;
    ULONG64                         KtxInterval;     // 100 ns
    ULONG64                         KtxStart;        // 100 ns interrupt time
    ULONG64                         KtxEnd;
    ULONG64                         KtxSent;
    ULONG64                         KtxCompleted;
    ULONG64                         KtxFailed;
    ULONG64                         KtxOut;
    ULONG64                         KtxNoSlot;
    ULONG64                         KtxFires;
    ULONG64                         KtxMaxBatch;
    ULONG64                         KtxMaxLate;
    // Media TX (IOCTL_DASIO_TX_SEND): the dasio service's frames, in the
    // test stream's slots and pool (they complete through ktxDone).
    ULONG64                         TxBatches;
    ULONG64                         TxFrames;
    ULONG64                         TxNoSlot;
    ULONG64                         TxMaxBatch;
    ULONG64                         KtxMaxOut;
    // TX_SEND calls inside txSendAtDispatch now: taken under FilterListLock,
    // dropped after the send, so a send does not hold the global lock and
    // detach waits for the count to reach 0 after unlinking.
    volatile LONG                   TxRefs;
    ULONG64                         KtxFailEvents;
    ULONG64                         KtxLastFailStatus;
    ULONG64                         KtxLastFailTime;

    // dasio deferred receive (rxd, dasio#1): received NBLs queued in
    // order (under Lock) and indicated up by RxdDpc.
    BOOLEAN                         RxdEnabled;
    LONG                            RxdCpu;
    KDPC                            RxdDpc;
    PNET_BUFFER_LIST                RxdHead;
    PNET_BUFFER_LIST               *RxdTail;
    ULONG                           RxdQueued;
    ULONG64                         RxdDeferred;
    ULONG64                         RxdPriority;
    ULONG64                         RxdInline;
    ULONG64                         RxdDpcRuns;
    ULONG64                         RxdMaxBatch;
    ULONG64                         RxdDropped;

}MS_FILTER, * PMS_FILTER;

// A split parent: the NBL tcpip sent and its children's bookkeeping. Each
// child's context area holds a pointer to it.
typedef struct _PACER_SPLIT_REC
{
    PNET_BUFFER_LIST    Parent;
    struct pacer_split  Split;
} PACER_SPLIT_REC, *PPACER_SPLIT_REC;

#define PACER_CHILD_CONTEXT         ((USHORT)MEMORY_ALLOCATION_ALIGNMENT)

// The pacer settings (filter.c) and applying them to one filter, under its
// Lock (IOCTL_DASIO_PACER_SET).
extern ULONG PacerRatePercent;
extern ULONG PacerBurstBytes;
VOID pacerApplyRateLocked(_In_ PMS_FILTER Filter);

// The kernel test stream (filter.c): start on this filter under its Lock
// (STATUS_DEVICE_NOT_READY when it has no slots or is not running), stop it
// (outside any lock: cancels the timer).
NTSTATUS ktxStartLocked(_In_ PMS_FILTER Filter, _In_ const DASIO_KTX_START *Start);
VOID ktxStop(_In_ PMS_FILTER Filter);

// Media TX (filter.c): sends the packed frames of `Input` (pacer_tx_next)
// on this filter, at DISPATCH_LEVEL with a TxRefs reference held (detach
// waits for it, so the filter stays attached), and sets *Accepted to the
// frames sent.
// STATUS_INVALID_PARAMETER for a malformed input (nothing sent),
// STATUS_DEVICE_NOT_READY when the filter has no slots or is not running.
// The bound on media frames out at the miniport (IOCTL_DASIO_TX_SET; from
// the service's Parameters TxMaxOut at load): 0 = every slot.
extern ULONG TxMaxOutSetting;

NTSTATUS txSendAtDispatch(_In_ PMS_FILTER Filter, _In_reads_bytes_(Length) const UCHAR *Input,
                          _In_ ULONG Length, _Out_ PULONG Accepted);

// Deferred receive (filter.c): settings for all filters, applied under
// each filter's Lock; the DPC's target CPU is set outside any lock.
extern ULONG RxdEnable;
extern LONG RxdCpuSetting;
VOID rxdApply(_In_ PMS_FILTER Filter);

// Other traffic goes at this share of the link speed...
#define PACER_RATE_PERCENT          85u
// ...in bursts of one timer tick (Windows timers: ~0.5 ms), so at most one
// tick of other data sits ahead of a priority frame.
#define PACER_TICK_100NS            5000u
#define PACER_MIN_BURST             16384u
// The burst other traffic gets by default (dasio#1 HIL 5: at 85 % of a
// 2.5 Gbit/s link, 16 KB keeps a priority packet's worst wait ~0.3 ms with
// full TCP throughput; one 0.5 ms tick, 133 KB, gave ~0.8 ms).
#define PACER_DEFAULT_BURST         16384u
// Header bytes the classifier reads: Ethernet, two VLAN tags, IPv6 or
// IPv4 with options, then the UDP ports.
#define PACER_HEADER_BYTES          128u


typedef struct _FILTER_DEVICE_EXTENSION
{
    ULONG            Signature;
    NDIS_HANDLE      Handle;
} FILTER_DEVICE_EXTENSION, *PFILTER_DEVICE_EXTENSION;


#define FILTER_READY_TO_PAUSE(_Filter)      \
    ((_Filter)->State == FilterPausing)

//
// The driver should maintain a list of NDIS filter handles
//
typedef struct _FL_NDIS_FILTER_LIST
{
    LIST_ENTRY              Link;
    NDIS_HANDLE             ContextHandle;
    NDIS_STRING             FilterInstanceName;
} FL_NDIS_FILTER_LIST, *PFL_NDIS_FILTER_LIST;

//
// The context inside a cloned request
//
typedef struct _NDIS_OID_REQUEST *FILTER_REQUEST_CONTEXT,**PFILTER_REQUEST_CONTEXT;


//
// function prototypes
//
DRIVER_INITIALIZE DriverEntry;

FILTER_SET_OPTIONS FilterRegisterOptions;

FILTER_ATTACH FilterAttach;

FILTER_DETACH FilterDetach;

DRIVER_UNLOAD FilterUnload;

FILTER_RESTART FilterRestart;

FILTER_PAUSE FilterPause;

FILTER_OID_REQUEST FilterOidRequest;

FILTER_CANCEL_OID_REQUEST FilterCancelOidRequest;

FILTER_STATUS FilterStatus;

FILTER_DEVICE_PNP_EVENT_NOTIFY FilterDevicePnPEventNotify;

FILTER_NET_PNP_EVENT FilterNetPnPEvent;

FILTER_OID_REQUEST_COMPLETE FilterOidRequestComplete;

FILTER_SEND_NET_BUFFER_LISTS FilterSendNetBufferLists;

FILTER_RETURN_NET_BUFFER_LISTS FilterReturnNetBufferLists;

FILTER_SEND_NET_BUFFER_LISTS_COMPLETE FilterSendNetBufferListsComplete;

FILTER_RECEIVE_NET_BUFFER_LISTS FilterReceiveNetBufferLists;

FILTER_CANCEL_SEND_NET_BUFFER_LISTS FilterCancelSendNetBufferLists;

FILTER_SET_MODULE_OPTIONS FilterSetModuleOptions;


_IRQL_requires_max_(PASSIVE_LEVEL)
NDIS_STATUS
FilterRegisterDevice(
    VOID
    );

_IRQL_requires_max_(PASSIVE_LEVEL)
VOID
FilterDeregisterDevice(
    VOID
    );

DRIVER_DISPATCH FilterDispatch;

DRIVER_DISPATCH FilterDeviceIoControl;

_IRQL_requires_max_(DISPATCH_LEVEL)
NDIS_STATUS
filterDoInternalRequest(
    _In_ PMS_FILTER                   FilterModuleContext,
    _In_ NDIS_REQUEST_TYPE            RequestType,
    _In_ NDIS_OID                     Oid,
    _Inout_updates_bytes_to_(InformationBufferLength, *pBytesProcessed)
         PVOID                        InformationBuffer,
    _In_ ULONG                        InformationBufferLength,
    _In_opt_ ULONG                    OutputBufferLength,
    _In_ ULONG                        MethodId,
    _Out_ PULONG                      pBytesProcessed
    );

VOID
filterInternalRequestComplete(
    _In_ NDIS_HANDLE                  FilterModuleContext,
    _In_ PNDIS_OID_REQUEST            NdisRequest,
    _In_ NDIS_STATUS                  Status
    );


#endif  //_FILT_H


