//
//    Copyright (C) Microsoft Corporation.  All rights reserved.
//
#ifndef __FILTERUSER_H__
#define __FILTERUSER_H__

//
// Temp file to test filter
//

#define _NDIS_CONTROL_CODE(request,method) \
            CTL_CODE(FILE_DEVICE_PHYSICAL_NETCARD, request, method, FILE_ANY_ACCESS)

#define IOCTL_FILTER_RESTART_ALL            _NDIS_CONTROL_CODE(0, METHOD_BUFFERED)
#define IOCTL_FILTER_RESTART_ONE_INSTANCE   _NDIS_CONTROL_CODE(1, METHOD_BUFFERED)
#define IOCTL_FILTER_ENUERATE_ALL_INSTANCES _NDIS_CONTROL_CODE(2, METHOD_BUFFERED)
#define IOCTL_FILTER_ENUMERATE_ALL_INSTANCES IOCTL_FILTER_ENUERATE_ALL_INSTANCES
#define IOCTL_FILTER_QUERY_ALL_STAT         _NDIS_CONTROL_CODE(3, METHOD_BUFFERED)
#define IOCTL_FILTER_CLEAR_ALL_STAT         _NDIS_CONTROL_CODE(4, METHOD_BUFFERED)
#define IOCTL_FILTER_SET_OID_VALUE          _NDIS_CONTROL_CODE(5, METHOD_BUFFERED)
#define IOCTL_FILTER_QUERY_OID_VALUE        _NDIS_CONTROL_CODE(6, METHOD_BUFFERED)
#define IOCTL_FILTER_CANCEL_REQUEST         _NDIS_CONTROL_CODE(7, METHOD_BUFFERED)
#define IOCTL_FILTER_READ_DRIVER_CONFIG     _NDIS_CONTROL_CODE(8, METHOD_BUFFERED)
#define IOCTL_FILTER_WRITE_DRIVER_CONFIG    _NDIS_CONTROL_CODE(9, METHOD_BUFFERED)
#define IOCTL_FILTER_READ_ADAPTER_CONFIG    _NDIS_CONTROL_CODE(10, METHOD_BUFFERED)
#define IOCTL_FILTER_WRITE_ADAPTER_CONFIG   _NDIS_CONTROL_CODE(11, METHOD_BUFFERED)
#define IOCTL_FILTER_READ_INSTANCE_CONFIG   _NDIS_CONTROL_CODE(12, METHOD_BUFFERED)
#define IOCTL_FILTER_WRITE_INSTANCE_CONFIG  _NDIS_CONTROL_CODE(13, METHOD_BUFFERED)
// dasio: the pacer's state, one DASIO_PACER_STATUS per bound adapter.
#define IOCTL_DASIO_PACER_STATUS            _NDIS_CONTROL_CODE(14, METHOD_BUFFERED)
// dasio: set the pacer's share of the link and its burst, for all bound
// adapters, live (input: DASIO_PACER_SET).
#define IOCTL_DASIO_PACER_SET               _NDIS_CONTROL_CODE(15, METHOD_BUFFERED)
// dasio: the kernel test stream (ktx, dasio#1): start one on the adapter
// with the given IfIndex (input: DASIO_KTX_START), stop all, read each
// bound adapter's DASIO_KTX_STATUS.
#define IOCTL_DASIO_KTX_START               _NDIS_CONTROL_CODE(16, METHOD_BUFFERED)
#define IOCTL_DASIO_KTX_STOP                _NDIS_CONTROL_CODE(17, METHOD_BUFFERED)
#define IOCTL_DASIO_KTX_STATUS              _NDIS_CONTROL_CODE(18, METHOD_BUFFERED)
// dasio: deferred receive (rxd, dasio#1): on or off for all bound adapters,
// and the CPU its DPC runs on (input: DASIO_RXD_SET); each bound adapter's
// DASIO_RXD_STATUS.
#define IOCTL_DASIO_RXD_SET                 _NDIS_CONTROL_CODE(19, METHOD_BUFFERED)
#define IOCTL_DASIO_RXD_STATUS              _NDIS_CONTROL_CODE(20, METHOD_BUFFERED)


#define MAX_FILTER_INSTANCE_NAME_LENGTH     256
#define MAX_FILTER_CONFIG_KEYWORD_LENGTH    256
typedef struct _FILTER_DRIVER_ALL_STAT
{
    ULONG          AttachCount;
    ULONG          DetachCount;
    ULONG          ExternalRequestFailedCount;
    ULONG          ExternalRequestSuccessCount;
    ULONG          InternalRequestFailedCount;
} FILTER_DRIVER_ALL_STAT, * PFILTER_DRIVER_ALL_STAT;


typedef struct _FILTER_SET_OID
{
    WCHAR           InstanceName[MAX_FILTER_INSTANCE_NAME_LENGTH];
    ULONG           InstanceNameLength;
    NDIS_OID        Oid;
    NDIS_STATUS     Status;
    UCHAR           Data[sizeof(ULONG)];

}FILTER_SET_OID, *PFILTER_SET_OID;

typedef struct _FILTER_QUERY_OID
{
    WCHAR           InstanceName[MAX_FILTER_INSTANCE_NAME_LENGTH];
    ULONG           InstanceNameLength;
    NDIS_OID        Oid;
    NDIS_STATUS     Status;
    UCHAR           Data[sizeof(ULONG)];

}FILTER_QUERY_OID, *PFILTER_QUERY_OID;

typedef struct _FILTER_READ_CONFIG
{
    _Field_size_bytes_part_(MAX_FILTER_INSTANCE_NAME_LENGTH,InstanceNameLength)
    WCHAR                   InstanceName[MAX_FILTER_INSTANCE_NAME_LENGTH];
    ULONG                   InstanceNameLength;
    _Field_size_bytes_part_(MAX_FILTER_CONFIG_KEYWORD_LENGTH,KeywordLength)
    WCHAR                   Keyword[MAX_FILTER_CONFIG_KEYWORD_LENGTH];
    ULONG                   KeywordLength;
    NDIS_PARAMETER_TYPE     ParameterType;
    NDIS_STATUS             Status;
    UCHAR                   Data[sizeof(ULONG)];
}FILTER_READ_CONFIG, *PFILTER_READ_CONFIG;

typedef struct _FILTER_WRITE_CONFIG
{
    _Field_size_bytes_part_(MAX_FILTER_INSTANCE_NAME_LENGTH,InstanceNameLength)
    WCHAR                   InstanceName[MAX_FILTER_INSTANCE_NAME_LENGTH];
    ULONG                   InstanceNameLength;
    _Field_size_bytes_part_(MAX_FILTER_CONFIG_KEYWORD_LENGTH,KeywordLength)
    WCHAR                   Keyword[MAX_FILTER_CONFIG_KEYWORD_LENGTH];
    ULONG                   KeywordLength;
    NDIS_PARAMETER_TYPE     ParameterType;
    NDIS_STATUS             Status;
    UCHAR                   Data[sizeof(ULONG)];
}FILTER_WRITE_CONFIG, *PFILTER_WRITE_CONFIG;

// dasio: one bound adapter's pacer, read under its lock. All fields 64 bit,
// so user mode reads it as an array of UINT64 in this order.
#define DASIO_PACER_STATUS_VERSION          3
typedef struct _DASIO_PACER_STATUS
{
    ULONG64         Version;
    ULONG64         IfIndex;
    ULONG64         State;            // FILTER_STATE: 4 = running
    ULONG64         LinkAtAttach;     // bits/s the attach reported
    ULONG64         LinkLast;         // bits/s the last link-state reported
    ULONG64         LinkUpdates;
    ULONG64         Rate;             // bytes/s, 0 = no pacing
    ULONG64         Burst;            // bytes
    LONG64          Tokens;
    ULONG64         Last;             // last refill, 100 ns
    ULONG64         Now;              // when read, 100 ns
    ULONG64         Queued;
    ULONG64         QueuedBytes;
    ULONG64         RingCount;
    ULONG64         MaxQueued;
    ULONG64         PrioritySent;
    ULONG64         Admitted;
    ULONG64         Deferred;
    ULONG64         TimerArms;
    ULONG64         TimerFires;
    ULONG64         TimerArmed;
    ULONG64         Pumps;
    ULONG64         PumpSent;
    ULONG64         Pumping;
    ULONG64         OtherNbls;        // non-priority NBLs seen
    ULONG64         OtherNbs;         // their NET_BUFFERs (packets)
    ULONG64         MaxNbs;           // most packets in one NBL
    ULONG64         MaxNblBytes;      // most bytes in one NBL
    ULONG64         Splits;           // multi-packet NBLs split
    ULONG64         SplitChildren;    // child NBLs made
    ULONG64         SplitFails;       // splits that could not allocate (sent whole)
    ULONG64         ChildrenOut;      // children not completed yet
} DASIO_PACER_STATUS, *PDASIO_PACER_STATUS;

// dasio: other traffic gets RatePercent of the link (0 = no pacing, at most
// 100) in bursts of BurstBytes (0 = one timer tick).
typedef struct _DASIO_PACER_SET
{
    ULONG           RatePercent;
    ULONG           BurstBytes;
} DASIO_PACER_SET, *PDASIO_PACER_SET;

// dasio: a kernel test stream of UDP frames the filter builds and sends
// itself, straight to the miniport (below tcpip and WFP). One frame every
// IntervalUs from the start for DurationS seconds; the payload starts with
// the sequence number and the send time (100 ns interrupt time), both
// 8-byte big endian. The source MAC is the adapter's own.
typedef struct _DASIO_KTX_START
{
    ULONG           IfIndex;
    UCHAR           DstMac[6];
    UCHAR           DstIp[4];
    UCHAR           SrcIp[4];
    USHORT          DstPort;
    USHORT          SrcPort;
    UCHAR           Dscp;
    UCHAR           Reserved[3];
    ULONG           PayloadBytes;     // 16..1472
    ULONG           IntervalUs;       // 50..1000000
    ULONG           DurationS;        // 1..86400
} DASIO_KTX_START, *PDASIO_KTX_START;

// dasio: one bound adapter's test stream, read under its lock. All fields
// 64 bit, read in this order.
#define DASIO_KTX_STATUS_VERSION            1
typedef struct _DASIO_KTX_STATUS
{
    ULONG64         Version;
    ULONG64         IfIndex;
    ULONG64         Active;
    ULONG64         Ready;            // its slots, pool and timer exist
    ULONG64         Start;            // 100 ns interrupt time
    ULONG64         End;
    ULONG64         Now;
    ULONG64         IntervalUs;
    ULONG64         Sent;             // frames handed to the miniport
    ULONG64         Completed;
    ULONG64         Failed;           // completed with an error status
    ULONG64         Out;              // not completed yet
    ULONG64         NoSlot;           // due frames dropped: every slot out
    ULONG64         Fires;
    ULONG64         MaxBatch;         // most frames sent in one fire
    ULONG64         MaxLate;          // 100 ns: a fire after its first due frame
} DASIO_KTX_STATUS, *PDASIO_KTX_STATUS;

// dasio: deferred receive. The miniport's receive DPC hands its NBLs to the
// filter, which queues them and returns at once; a DPC of the filter's own,
// on Cpu, indicates them up. tcpip's and WFP's work for a received packet
// then runs there, not inside the NIC's DPC (with Realtek 10.72/10.79 the
// NIC sends nothing while its DPC runs, dasio#1). Only TCP and discovery
// chatter (mDNS, LLMNR, NetBIOS, SSDP, broadcast) are deferred; Dante
// audio of any DSCP, PTP and the rest go up at once. Cpu -1 = the
// receiving CPU. Off after a restart of the filter.
typedef struct _DASIO_RXD_SET
{
    ULONG           Enable;
    LONG            Cpu;
} DASIO_RXD_SET, *PDASIO_RXD_SET;

#define DASIO_RXD_STATUS_VERSION            2
typedef struct _DASIO_RXD_STATUS
{
    ULONG64         Version;
    ULONG64         IfIndex;
    ULONG64         Enabled;
    LONG64          Cpu;
    ULONG64         Deferred;         // NBLs queued and indicated by our DPC
    ULONG64         Inline;           // NBLs indicated at once (cannot pend, or off)
    ULONG64         DpcRuns;
    ULONG64         MaxBatch;         // most NBLs one DPC run indicated
    ULONG64         Queued;           // waiting now
    ULONG64         Dropped;          // returned unindicated (pause)
    ULONG64         Priority;         // NBLs indicated at once while rxd is on (not deferred)
} DASIO_RXD_STATUS, *PDASIO_RXD_STATUS;

#endif //__FILTERUSER_H__

