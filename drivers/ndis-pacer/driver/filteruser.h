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
// dasio: media TX (dasio#1): the dasio service hands its own audio frames
// down (input: DASIO_TX_SEND, then the packed frames; output: one ULONG,
// the frames accepted); the filter sends them straight to the miniport,
// below tcpip and WFP. Write access only (the device is SYSTEM and
// Administrators only).
#define IOCTL_DASIO_TX_SEND                 CTL_CODE(FILE_DEVICE_PHYSICAL_NETCARD, 21, METHOD_BUFFERED, FILE_WRITE_ACCESS)
// dasio: media TX's bound on frames out at the miniport, for all bound
// adapters, live (input: one ULONG, 1..4096; 0, once every slot, is
// refused: media TX is never unbounded). Write access only.
#define IOCTL_DASIO_TX_SET                  CTL_CODE(FILE_DEVICE_PHYSICAL_NETCARD, 22, METHOD_BUFFERED, FILE_WRITE_ACCESS)
// dasio: kernel RX (dasio#27, B23): received frames of the caller's own
// flows copied into a ring mapped into its process, before tcpip. One ring
// per open handle, on one adapter: RX_OPEN makes it (input DASIO_RX_OPEN,
// output DASIO_RX_OPENED), RX_MATCH replaces its match table (input
// DASIO_RX_MATCH), RX_CLOSE closes it (closing the handle does too); write
// access only. RX_STATUS: one DASIO_RX_STATUS per open ring. The ring's
// layout is in ../core/rx_ring.h. RX_OPEN answers STATUS_NOT_SUPPORTED
// while the service's Parameters\RxRingEnable is 0 (the default), and an
// older filter answers these codes with success and no output: check the
// output's size.
// The reader's contract:
// - open the device handle non-inheritable (no child process may hold a
//   duplicate of it);
// - stop reading before RX_CLOSE or closing the handle: the mapping goes
//   away at once;
// - a second RX_OPEN on the same handle needs RX_CLOSE first;
// - a ring whose owner process is gone while a duplicated handle lives on
//   (an orphan) stays attached, and in steal mode keeps its flows from
//   tcpip, until that handle closes.
#define IOCTL_DASIO_RX_OPEN                 CTL_CODE(FILE_DEVICE_PHYSICAL_NETCARD, 23, METHOD_BUFFERED, FILE_WRITE_ACCESS)
#define IOCTL_DASIO_RX_MATCH                CTL_CODE(FILE_DEVICE_PHYSICAL_NETCARD, 24, METHOD_BUFFERED, FILE_WRITE_ACCESS)
#define IOCTL_DASIO_RX_CLOSE                CTL_CODE(FILE_DEVICE_PHYSICAL_NETCARD, 25, METHOD_BUFFERED, FILE_WRITE_ACCESS)
#define IOCTL_DASIO_RX_STATUS               _NDIS_CONTROL_CODE(26, METHOD_BUFFERED)


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
#define DASIO_KTX_STATUS_VERSION            4
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
    ULONG64         Sent;             // test stream frames handed to the miniport
    ULONG64         Completed;        // every frame of the slots (stream and media)
    ULONG64         Failed;           // completed with an error status (stream and media)
    ULONG64         Out;              // not completed yet
    ULONG64         NoSlot;           // due frames dropped: every slot out
    ULONG64         Fires;
    ULONG64         MaxBatch;         // most frames sent in one fire
    ULONG64         MaxLate;          // 100 ns: a fire after its first due frame
    // Version 2: media TX (IOCTL_DASIO_TX_SEND), sharing the slots;
    // Completed, Failed and Out above count its frames too, Sent does not.
    ULONG64         TxBatches;        // TX_SEND calls that sent a frame
    ULONG64         TxFrames;         // frames accepted and sent
    ULONG64         TxNoSlot;         // frames not accepted: every slot out
    ULONG64         TxMaxBatch;       // most frames in one call
    // Version 3: the bound on media frames out, and the failed completions
    // (the miniport refused the frame: lost).
    ULONG64         TxMaxOut;         // the bound: 1..4096 (200 when Parameters has none or an invalid one)
    ULONG64         MaxOut;           // most frames out at once, stream and media
    ULONG64         FailEvents;       // failures more than 1 s after the previous one
    ULONG64         LastFailStatus;   // NDIS_STATUS of the last failure
    ULONG64         LastFailTime;     // 100 ns interrupt time of the last failure
    // Version 4: media frames handed back to the socket (TxNoSlot).
    ULONG64         NoSlotEvents;     // TxNoSlot bursts more than 1 s after the previous one
    ULONG64         LastNoSlotTime;   // 100 ns interrupt time of the last one
} DASIO_KTX_STATUS, *PDASIO_KTX_STATUS;

// dasio: media TX. IfIndex names the adapter; the frames follow this
// header, each as a 2-byte little-endian length and an Ethernet II frame
// carrying IPv4 (pacer_tx_next). The filter writes the adapter's own MAC
// as each frame's source. Frames are accepted in order while the
// filter has free slots: the output's count says how many, and the rest
// were not sent (the caller sends them another way). A malformed input
// sends nothing (STATUS_INVALID_PARAMETER).
typedef struct _DASIO_TX_SEND
{
    ULONG           IfIndex;
    ULONG           Reserved;
} DASIO_TX_SEND, *PDASIO_TX_SEND;

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

#define DASIO_RXD_STATUS_VERSION            3
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
    // Version 3: the service's Parameters values that were there but could
    // not be read when the driver loaded (another type than REG_DWORD), or
    // were invalid (TxMaxOut 0 or above 4096): bit 0 RxdEnable, 1 RxdCpu,
    // 2 TxMaxOut, 3 RxRingEnable. Each kept its default (TxMaxOut: 200);
    // 0 = all read or missing.
    ULONG64         ParamBad;
} DASIO_RXD_STATUS, *PDASIO_RXD_STATUS;
// Version 2: 11 fields, version 3: 12 (rxd.ps1 reads both).
C_ASSERT(sizeof(DASIO_RXD_STATUS) == 12 * sizeof(ULONG64));

// dasio: kernel RX. A ring of SlotCount slots (a power of 2, 64..4096) on
// the adapter with the given IfIndex, mapped into the calling process.
// Event: a handle of an auto-reset event of the caller's
// (EVENT_MODIFY_STATE) the filter sets when the ring's wake word is
// non-zero after a frame, and when it detaches the ring (the header's
// detached word is then 1: close and open again), or 0. Reserved: 0.
// Errors: STATUS_NOT_SUPPORTED (RxRingEnable 0), STATUS_NOT_FOUND (no
// adapter with that IfIndex), STATUS_DEVICE_NOT_READY (the filter is not
// running), STATUS_INVALID_DEVICE_STATE (this handle has a ring),
// STATUS_INSUFFICIENT_RESOURCES (memory, or RX_RINGS_PER_FILTER rings on
// that adapter already), STATUS_INVALID_PARAMETER.
// Copy mode (default): a matched frame goes into the ring and up to tcpip
// as before. DASIO_RX_STEAL: a frame every matching ring steals does not
// go up (a full ring drops it: Full); one longer than a slot always goes up.
// A tagged frame (a VLAN ID in a tag in it, or a non-zero VLAN ID the NIC
// took off; a priority tag, VLAN ID 0, is untagged) or one whose IP or UDP
// checksum the NIC found bad never goes into a ring and always goes up
// (Tagged, BadChecksum).
#define DASIO_RX_STEAL                      0x00000001u
#define DASIO_RX_FLAGS                      DASIO_RX_STEAL
typedef struct _DASIO_RX_OPEN
{
    ULONG           IfIndex;
    ULONG           SlotCount;
    ULONG           Flags;
    ULONG           Reserved;
    ULONG64         Event;            // HANDLE, 0 = none
} DASIO_RX_OPEN, *PDASIO_RX_OPEN;

// Where the ring is in the caller's process, and its bytes.
typedef struct _DASIO_RX_OPENED
{
    ULONG64         Address;
    ULONG64         Bytes;
} DASIO_RX_OPENED, *PDASIO_RX_OPENED;

// The ring's whole match table, replaced at once: Count entries (0..16)
// of a destination IPv4 address (as on the wire, a.b.c.d) and UDP port
// (a number, not 0). Address 0.0.0.0: any unicast address on that port in
// a frame to a unicast MAC (never a subnet broadcast; another host's
// flooded unicast too: name this PC's own address to exclude it); a
// multicast group matches only itself. Only the first
// Count entries need to be in the input; every Reserved field 0.
#define DASIO_RX_MAX_MATCHES                16
typedef struct _DASIO_RX_MATCH_ENTRY
{
    UCHAR           Address[4];
    USHORT          Port;
    USHORT          Reserved;
} DASIO_RX_MATCH_ENTRY, *PDASIO_RX_MATCH_ENTRY;

typedef struct _DASIO_RX_MATCH
{
    ULONG           Count;
    ULONG           Reserved;
    DASIO_RX_MATCH_ENTRY Entries[DASIO_RX_MAX_MATCHES];
} DASIO_RX_MATCH, *PDASIO_RX_MATCH;

// One open ring, read under its adapter's RX ring lock. All fields 64
// bit, read in this order. Version 2: Tagged and BadChecksum (version 1,
// without them, never left dev).
#define DASIO_RX_STATUS_VERSION             2
typedef struct _DASIO_RX_STATUS
{
    ULONG64         Version;
    ULONG64         IfIndex;
    ULONG64         Attached;         // 0 after a pause or detach of the filter: reopen
    ULONG64         Flags;
    ULONG64         SlotCount;
    ULONG64         Matches;          // entries in its match table
    ULONG64         Matched;          // frames its table matched: Delivered + Full + TooBig
    ULONG64         Delivered;        // put into the ring
    ULONG64         Full;             // not put: every slot unread
    ULONG64         Stolen;           // kept from tcpip (steal mode)
    ULONG64         TooBig;           // not put: longer than a slot, or unreadable
    // Version 2:
    ULONG64         Tagged;           // its table named it, but it was VLAN-tagged: up, not put
    ULONG64         BadChecksum;      // its table named it, but its checksum was bad: up, not put
} DASIO_RX_STATUS, *PDASIO_RX_STATUS;
// Version 1: 11 fields, version 2: 13 (rxring.ps1 reads both).
C_ASSERT(sizeof(DASIO_RX_STATUS) == 13 * sizeof(ULONG64));

#endif //__FILTERUSER_H__

