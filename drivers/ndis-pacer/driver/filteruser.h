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
#define DASIO_PACER_STATUS_VERSION          2
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
} DASIO_PACER_STATUS, *PDASIO_PACER_STATUS;

// dasio: other traffic gets RatePercent of the link (0 = no pacing, at most
// 100) in bursts of BurstBytes (0 = one timer tick).
typedef struct _DASIO_PACER_SET
{
    ULONG           RatePercent;
    ULONG           BurstBytes;
} DASIO_PACER_SET, *PDASIO_PACER_SET;

#endif //__FILTERUSER_H__

