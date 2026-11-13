/*++

Copyright (c) Microsoft Corporation

Module Name:

    Filter.c

Abstract:

    Sample NDIS Lightweight filter driver

--*/

#include "precomp.h"

// dasio send pacer helpers (defined before FilterSendNetBufferListsComplete).
static ULONG pacerNblBytes(_In_ PNET_BUFFER_LIST Nbl);
static int pacerNblRxDefer(_In_ PNET_BUFFER_LIST Nbl);
static enum pacer_class pacerNblClass(_In_ PNET_BUFFER_LIST Nbl);
static VOID pacerPump(_In_ PMS_FILTER Filter, _In_ BOOLEAN DispatchLevel, _In_ BOOLEAN FromTimer);
static ULONG64 pacerNow(VOID);
static VOID pacerRateFor(_In_ ULONG64 LinkBitsPerSecond, _Out_ ULONG64 *Rate, _Out_ ULONG *Burst);
static VOID pacerArmLocked(_In_ PMS_FILTER Filter, _In_ ULONG64 Now);
static EXT_CALLBACK pacerTimer;
static EXT_CALLBACK ktxTimer;
static KDEFERRED_ROUTINE rxdDpc;
static VOID rxdDrop(_In_ PMS_FILTER Filter);
static VOID rxdReadRegistry(_In_ PUNICODE_STRING RegistryPath);
static VOID ktxSetup(_In_ PMS_FILTER Filter, _In_ PNDIS_FILTER_ATTACH_PARAMETERS AttachParameters);
static VOID ktxTeardown(_In_ PMS_FILTER Filter);
static VOID ktxDone(_In_ PMS_FILTER Filter, _In_ PNET_BUFFER_LIST Nbl, _In_ BOOLEAN DispatchLevel);
static VOID pacerComplete(_In_ PMS_FILTER Filter, _In_opt_ PNET_BUFFER_LIST Nbls, _In_ BOOLEAN SetStatus, _In_ NDIS_STATUS Status, _In_ BOOLEAN DispatchLevel, _In_ ULONG UpFlags);

#define __FILENUMBER    'PNPF'

// This directive puts the DriverEntry function into the INIT segment of the
// driver.  To conserve memory, the code will be discarded when the driver's
// DriverEntry function returns.  You can declare other functions used only
// during initialization here.
#pragma NDIS_INIT_FUNCTION(DriverEntry)

//
// Global variables
//
NDIS_HANDLE         FilterDriverHandle; // NDIS handle for filter driver
NDIS_HANDLE         FilterDriverObject;
NDIS_HANDLE         NdisFilterDeviceHandle = NULL;
PDEVICE_OBJECT      NdisDeviceObject = NULL;

FILTER_LOCK         FilterListLock;
LIST_ENTRY          FilterModuleList;

NDIS_FILTER_PARTIAL_CHARACTERISTICS DefaultChars = {
{ 0, 0, 0},
      0,
      FilterSendNetBufferLists,
      FilterSendNetBufferListsComplete,
      NULL,
      FilterReceiveNetBufferLists,
      FilterReturnNetBufferLists
};


_Use_decl_annotations_
NTSTATUS
DriverEntry(
    PDRIVER_OBJECT      DriverObject,
    PUNICODE_STRING     RegistryPath
    )
/*++

Routine Description:

    First entry point to be called, when this driver is loaded.
    Register with NDIS as a filter driver and create a device
    for communication with user-mode.

Arguments:

    DriverObject - pointer to the system's driver object structure
                   for this driver

    RegistryPath - system's registry path for this driver

Return Value:

    STATUS_SUCCESS if all initialization is successful, STATUS_XXX
    error code if not.

--*/
{
    NDIS_STATUS Status;
    NDIS_FILTER_DRIVER_CHARACTERISTICS      FChars;
    NDIS_STRING ServiceName  = RTL_CONSTANT_STRING(FILTER_SERVICE_NAME);
    NDIS_STRING UniqueName   = RTL_CONSTANT_STRING(FILTER_UNIQUE_NAME);
    NDIS_STRING FriendlyName = RTL_CONSTANT_STRING(FILTER_FRIENDLY_NAME);
    BOOLEAN bFalse = FALSE;

    DEBUGP(DL_TRACE, "===>DriverEntry...\n");

    // Deferred receive from the service's Parameters key (RxdEnable,
    // RxdCpu), before any attach applies it.
    rxdReadRegistry(RegistryPath);

    FilterDriverObject = DriverObject;

    do
    {
        NdisZeroMemory(&FChars, sizeof(NDIS_FILTER_DRIVER_CHARACTERISTICS));
        FChars.Header.Type = NDIS_OBJECT_TYPE_FILTER_DRIVER_CHARACTERISTICS;
        FChars.Header.Size = sizeof(NDIS_FILTER_DRIVER_CHARACTERISTICS);
#if NDIS_SUPPORT_NDIS61
        FChars.Header.Revision = NDIS_FILTER_CHARACTERISTICS_REVISION_2;
#else
        FChars.Header.Revision = NDIS_FILTER_CHARACTERISTICS_REVISION_1;
#endif

        FChars.MajorNdisVersion = FILTER_MAJOR_NDIS_VERSION;
        FChars.MinorNdisVersion = FILTER_MINOR_NDIS_VERSION;
        FChars.MajorDriverVersion = 1;
        FChars.MinorDriverVersion = 0;
        FChars.Flags = 0;

        FChars.FriendlyName = FriendlyName;
        FChars.UniqueName = UniqueName;
        FChars.ServiceName = ServiceName;

        //
        // TODO: Most handlers are optional, however, this sample includes them
        // all for illustrative purposes.  If you do not need a particular
        // handler, set it to NULL and NDIS will more efficiently pass the
        // operation through on your behalf.
        //
        FChars.SetOptionsHandler = FilterRegisterOptions;
        FChars.AttachHandler = FilterAttach;
        FChars.DetachHandler = FilterDetach;
        FChars.RestartHandler = FilterRestart;
        FChars.PauseHandler = FilterPause;
        FChars.SetFilterModuleOptionsHandler = FilterSetModuleOptions;
        FChars.OidRequestHandler = FilterOidRequest;
        FChars.OidRequestCompleteHandler = FilterOidRequestComplete;
        FChars.CancelOidRequestHandler = FilterCancelOidRequest;

        FChars.SendNetBufferListsHandler = FilterSendNetBufferLists;
        FChars.ReturnNetBufferListsHandler = FilterReturnNetBufferLists;
        FChars.SendNetBufferListsCompleteHandler = FilterSendNetBufferListsComplete;
        FChars.ReceiveNetBufferListsHandler = FilterReceiveNetBufferLists;
        FChars.DevicePnPEventNotifyHandler = FilterDevicePnPEventNotify;
        FChars.NetPnPEventHandler = FilterNetPnPEvent;
        FChars.StatusHandler = FilterStatus;
        FChars.CancelSendNetBufferListsHandler = FilterCancelSendNetBufferLists;

        DriverObject->DriverUnload = FilterUnload;

        FilterDriverHandle = NULL;

        //
        // Initialize spin locks
        //
        FILTER_INIT_LOCK(&FilterListLock);

        InitializeListHead(&FilterModuleList);

        Status = NdisFRegisterFilterDriver(DriverObject,
                                           (NDIS_HANDLE)FilterDriverObject,
                                           &FChars,
                                           &FilterDriverHandle);
        if (Status != NDIS_STATUS_SUCCESS)
        {
            DEBUGP(DL_WARN, "Register filter driver failed.\n");
            break;
        }

        Status = FilterRegisterDevice();

        if (Status != NDIS_STATUS_SUCCESS)
        {
            NdisFDeregisterFilterDriver(FilterDriverHandle);
            FILTER_FREE_LOCK(&FilterListLock);
            DEBUGP(DL_WARN, "Register device for the filter driver failed.\n");
            break;
        }


    }
    while(bFalse);


    DEBUGP(DL_TRACE, "<===DriverEntry, Status = %8x\n", Status);
    return Status;

}

_Use_decl_annotations_
NDIS_STATUS
FilterRegisterOptions(
    NDIS_HANDLE  NdisFilterDriverHandle,
    NDIS_HANDLE  FilterDriverContext
    )
/*++

Routine Description:

    Register optional handlers with NDIS.  This sample does not happen to
    have any optional handlers to register, so this routine does nothing
    and could simply have been omitted.  However, for illustrative purposes,
    it is presented here.

Arguments:

    NdisFilterDriverHandle - pointer the driver handle received from
                             NdisFRegisterFilterDriver

    FilterDriverContext    - pointer to our context passed into
                             NdisFRegisterFilterDriver

Return Value:

    NDIS_STATUS_SUCCESS

--*/
{
    DEBUGP(DL_TRACE, "===>FilterRegisterOptions\n");

    ASSERT(NdisFilterDriverHandle == FilterDriverHandle);
    ASSERT(FilterDriverContext == (NDIS_HANDLE)FilterDriverObject);

    if ((NdisFilterDriverHandle != (NDIS_HANDLE)FilterDriverHandle) ||
        (FilterDriverContext != (NDIS_HANDLE)FilterDriverObject))
    {
        return NDIS_STATUS_INVALID_PARAMETER;
    }

    DEBUGP(DL_TRACE, "<===FilterRegisterOptions\n");

    return NDIS_STATUS_SUCCESS;
}


_Use_decl_annotations_
NDIS_STATUS
FilterAttach(
    NDIS_HANDLE                     NdisFilterHandle,
    NDIS_HANDLE                     FilterDriverContext,
    PNDIS_FILTER_ATTACH_PARAMETERS  AttachParameters
    )
/*++

Routine Description:

    Filter attach routine.
    Create filter's context, allocate NetBufferLists and NetBuffer pools and any
    other resources, and read configuration if needed.

Arguments:

    NdisFilterHandle - Specify a handle identifying this instance of the filter. FilterAttach
                       should save this handle. It is a required  parameter in subsequent calls
                       to NdisFxxx functions.
    FilterDriverContext - Filter driver context passed to NdisFRegisterFilterDriver.

    AttachParameters - attach parameters

Return Value:

    NDIS_STATUS_SUCCESS: FilterAttach successfully allocated and initialize data structures
                         for this filter instance.
    NDIS_STATUS_RESOURCES: FilterAttach failed due to insufficient resources.
    NDIS_STATUS_FAILURE: FilterAttach could not set up this instance of this filter and it has called
                         NdisWriteErrorLogEntry with parameters specifying the reason for failure.

N.B.:  FILTER can use NdisRegisterDeviceEx to create a device, so the upper
    layer can send Irps to the filter.

--*/
{
    PMS_FILTER              pFilter = NULL;
    NDIS_STATUS             Status = NDIS_STATUS_SUCCESS;
    NDIS_FILTER_ATTRIBUTES  FilterAttributes;
    ULONG                   Size;
    BOOLEAN               bFalse = FALSE;

    DEBUGP(DL_TRACE, "===>FilterAttach: NdisFilterHandle %p\n", NdisFilterHandle);

    do
    {
        ASSERT(FilterDriverContext == (NDIS_HANDLE)FilterDriverObject);
        if (FilterDriverContext != (NDIS_HANDLE)FilterDriverObject)
        {
            Status = NDIS_STATUS_INVALID_PARAMETER;
            break;
        }

        // Verify the media type is supported.  This is a last resort; the
        // the filter should never have been bound to an unsupported miniport
        // to begin with.  If this driver is marked as a Mandatory filter (which
        // is the default for this sample; see the INF file), failing to attach
        // here will leave the network adapter in an unusable state.
        //
        // Your setup/install code should not bind the filter to unsupported
        // media types.
        if ((AttachParameters->MiniportMediaType != NdisMedium802_3)
                && (AttachParameters->MiniportMediaType != NdisMediumWan)
                && (AttachParameters->MiniportMediaType != NdisMediumWirelessWan))
        {
           DEBUGP(DL_ERROR, "Unsupported media type.\n");

           Status = NDIS_STATUS_INVALID_PARAMETER;
           break;
        }

        Size = sizeof(MS_FILTER) +
               AttachParameters->FilterModuleGuidName->Length +
               AttachParameters->BaseMiniportInstanceName->Length +
               AttachParameters->BaseMiniportName->Length;

        pFilter = (PMS_FILTER)FILTER_ALLOC_MEM(NdisFilterHandle, Size);
        if (pFilter == NULL)
        {
            DEBUGP(DL_WARN, "Failed to allocate context structure.\n");
            Status = NDIS_STATUS_RESOURCES;
            break;
        }

        NdisZeroMemory(pFilter, sizeof(MS_FILTER));

        pFilter->FilterModuleName.Length = pFilter->FilterModuleName.MaximumLength = AttachParameters->FilterModuleGuidName->Length;
        pFilter->FilterModuleName.Buffer = (PWSTR)((PUCHAR)pFilter + sizeof(MS_FILTER));
        NdisMoveMemory(pFilter->FilterModuleName.Buffer,
                        AttachParameters->FilterModuleGuidName->Buffer,
                        pFilter->FilterModuleName.Length);



        pFilter->MiniportFriendlyName.Length = pFilter->MiniportFriendlyName.MaximumLength = AttachParameters->BaseMiniportInstanceName->Length;
        pFilter->MiniportFriendlyName.Buffer = (PWSTR)((PUCHAR)pFilter->FilterModuleName.Buffer + pFilter->FilterModuleName.Length);
        NdisMoveMemory(pFilter->MiniportFriendlyName.Buffer,
                        AttachParameters->BaseMiniportInstanceName->Buffer,
                        pFilter->MiniportFriendlyName.Length);


        pFilter->MiniportName.Length = pFilter->MiniportName.MaximumLength = AttachParameters->BaseMiniportName->Length;
        pFilter->MiniportName.Buffer = (PWSTR)((PUCHAR)pFilter->MiniportFriendlyName.Buffer +
                                                   pFilter->MiniportFriendlyName.Length);
        NdisMoveMemory(pFilter->MiniportName.Buffer,
                        AttachParameters->BaseMiniportName->Buffer,
                        pFilter->MiniportName.Length);

        pFilter->MiniportIfIndex = AttachParameters->BaseMiniportIfIndex;
        //
        // The filter should initialize TrackReceives and TrackSends properly. For this
        // driver, since its default characteristic has both a send and a receive handler,
        // these fields are initialized to TRUE.
        //
        pFilter->TrackReceives = TRUE;
        pFilter->TrackSends = TRUE;
        pFilter->FilterHandle = NdisFilterHandle;


        NdisZeroMemory(&FilterAttributes, sizeof(NDIS_FILTER_ATTRIBUTES));
        FilterAttributes.Header.Revision = NDIS_FILTER_ATTRIBUTES_REVISION_1;
        FilterAttributes.Header.Size = sizeof(NDIS_FILTER_ATTRIBUTES);
        FilterAttributes.Header.Type = NDIS_OBJECT_TYPE_FILTER_ATTRIBUTES;
        FilterAttributes.Flags = 0;

        NDIS_DECLARE_FILTER_MODULE_CONTEXT(MS_FILTER);
        Status = NdisFSetAttributes(NdisFilterHandle,
                                    pFilter,
                                    &FilterAttributes);
        if (Status != NDIS_STATUS_SUCCESS)
        {
            DEBUGP(DL_WARN, "Failed to set attributes.\n");
            break;
        }


        {
            ULONG64 Rate;
            ULONG Burst;
            pacerRateFor(AttachParameters->XmitLinkSpeed, &Rate, &Burst);
            pFilter->PaceTimer = ExAllocateTimer(pacerTimer, pFilter, EX_TIMER_HIGH_RESOLUTION);
            pFilter->PaceRing = (PPACER_ENTRY)NdisAllocateMemoryWithTagPriority(
                FilterDriverHandle, sizeof(PACER_ENTRY) * PACER_RING_SIZE, FILTER_ALLOC_TAG, NormalPoolPriority);
            pFilter->PaceRingHead = 0;
            pFilter->PaceRingCount = 0;
            if (pFilter->PaceTimer == NULL || pFilter->PaceRing == NULL)
            {
                // No timer or ring, no pacing: nothing may wait without them.
                Rate = 0;
                Burst = 0;
            }
            pacer_init(&pFilter->Pacer, Rate, Burst, pacerNow());
            pFilter->PaceTimerArmed = FALSE;
            pFilter->PacePumping = FALSE;
            {
                NET_BUFFER_LIST_POOL_PARAMETERS PoolParams;
                NdisZeroMemory(&PoolParams, sizeof(PoolParams));
                PoolParams.Header.Type = NDIS_OBJECT_TYPE_DEFAULT;
                PoolParams.Header.Revision = NET_BUFFER_LIST_POOL_PARAMETERS_REVISION_1;
                PoolParams.Header.Size = NDIS_SIZEOF_NET_BUFFER_LIST_POOL_PARAMETERS_REVISION_1;
                PoolParams.ProtocolId = NDIS_PROTOCOL_ID_DEFAULT;
                PoolParams.fAllocateNetBuffer = TRUE;
                PoolParams.ContextSize = PACER_CHILD_CONTEXT;
                PoolParams.PoolTag = FILTER_ALLOC_TAG;
                PoolParams.DataSize = 0;
                // No pool, no split: a multi-packet NBL then goes as a whole.
                pFilter->PaceNblPool = NdisAllocateNetBufferListPool(pFilter->FilterHandle, &PoolParams);
            }
            pFilter->PaceLinkAtAttach = AttachParameters->XmitLinkSpeed;
            pFilter->PaceLinkLast = AttachParameters->XmitLinkSpeed;
        }
        // The kernel test stream's slots, pool and timer (none: no stream).
        ktxSetup(pFilter, AttachParameters);
        // Deferred receive: its DPC and empty queue, applied from the
        // settings (off by default).
        KeInitializeDpc(&pFilter->RxdDpc, rxdDpc, pFilter);
        pFilter->RxdHead = NULL;
        pFilter->RxdTail = &pFilter->RxdHead;
        pFilter->RxdCpu = -1;
        rxdApply(pFilter);
        pFilter->State = FilterPaused;

        FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
        InsertHeadList(&FilterModuleList, &pFilter->FilterModuleLink);
        FILTER_RELEASE_LOCK(&FilterListLock, bFalse);

    }
    while (bFalse);

    if (Status != NDIS_STATUS_SUCCESS)
    {
        if (pFilter != NULL)
        {
            FILTER_FREE_MEM(pFilter);
        }
    }

    DEBUGP(DL_TRACE, "<===FilterAttach:    Status %x\n", Status);
    return Status;
}

_Use_decl_annotations_
NDIS_STATUS
FilterPause(
    NDIS_HANDLE                     FilterModuleContext,
    PNDIS_FILTER_PAUSE_PARAMETERS   PauseParameters
    )
/*++

Routine Description:

    Filter pause routine.
    Complete all the outstanding sends and queued sends,
    wait for all the outstanding recvs to be returned
    and return all the queued receives.

Arguments:

    FilterModuleContext - pointer to the filter context stucture
    PauseParameters     - additional information about the pause

Return Value:

    NDIS_STATUS_SUCCESS if filter pauses successfully, NDIS_STATUS_PENDING
    if not.  No other return value is allowed (pause must succeed, eventually).

N.B.: When the filter is in Pausing state, it can still process OID requests,
    complete sending, and returning packets to NDIS, and also indicate status.
    After this function completes, the filter must not attempt to send or
    receive packets, but it may still process OID requests and status
    indications.

--*/
{
    PMS_FILTER          pFilter = (PMS_FILTER)(FilterModuleContext);
    NDIS_STATUS         Status;
    BOOLEAN               bFalse = FALSE;

    UNREFERENCED_PARAMETER(PauseParameters);

    DEBUGP(DL_TRACE, "===>NDISLWF FilterPause: FilterInstance %p\n", FilterModuleContext);

    //
    // Set the flag that the filter is going to pause
    //
    FILTER_ASSERT(pFilter->State == FilterRunning);

    FILTER_ACQUIRE_LOCK(&pFilter->Lock, bFalse);
    pFilter->State = FilterPausing;
    FILTER_RELEASE_LOCK(&pFilter->Lock, bFalse);

    // The kernel test stream sends nothing more; its frames that are out
    // complete before pause returns (the wait below).
    ktxStop(pFilter);
    // Deferred receive: what waits goes back to the miniport unindicated,
    // and a DPC that already runs finishes (KeFlushQueuedDpcs below).
    rxdDrop(pFilter);

    //
    // Do whatever work is required to bring the filter into the Paused state.
    //
    // If you have diverted and queued any send or receive NBLs, return them
    // now.
    //
    // If you send or receive original NBLs, stop doing that and wait for your
    // NBLs to return to you now.
    //


    //
    // dasio send pacer: the NBLs waiting here never went down; they
    // complete upwards as paused.
    //
    {
        PNET_BUFFER_LIST Queued;
        ULONG Count = 0;
        ULONG64 Bytes = 0;

        FILTER_ACQUIRE_LOCK(&pFilter->Lock, bFalse);
        Queued = NULL;
        {
            PNET_BUFFER_LIST *Tail = &Queued;
            while (pFilter->PaceRingCount > 0)
            {
                PNET_BUFFER_LIST Nbl = pFilter->PaceRing[pFilter->PaceRingHead].Nbl;
                pFilter->PaceRingHead = (pFilter->PaceRingHead + 1) % PACER_RING_SIZE;
                pFilter->PaceRingCount--;
                NET_BUFFER_LIST_NEXT_NBL(Nbl) = NULL;
                *Tail = Nbl;
                Tail = &NET_BUFFER_LIST_NEXT_NBL(Nbl);
                Count++;
                Bytes += pacerNblBytes(Nbl);
            }
        }
        pacer_drop_queued(&pFilter->Pacer, Count, Bytes);
        pFilter->PaceTimerArmed = FALSE;
        FILTER_RELEASE_LOCK(&pFilter->Lock, bFalse);

        if (pFilter->PaceTimer != NULL)
        {
            ExCancelTimer(pFilter->PaceTimer, NULL);
        }
        pacerComplete(pFilter, Queued, TRUE, NDIS_STATUS_PAUSED, FALSE, 0);

        // A timer callback or a pump that already runs may still hold NBLs
        // it took: paused means none of ours goes down any more. The state
        // is not running, so a pump that starts now takes nothing.
        KeFlushQueuedDpcs();
        for (;;)
        {
            ULONG Pumping;
            ULONG64 ChildrenOut;
            ULONG64 KtxOut;
            FILTER_ACQUIRE_LOCK(&pFilter->Lock, bFalse);
            Pumping = pFilter->PacePumping;
            ChildrenOut = pFilter->PaceChildrenOut;
            KtxOut = pFilter->KtxOut;
            FILTER_RELEASE_LOCK(&pFilter->Lock, bFalse);
            // Our child NBLs below complete to us; their parents go up
            // with the last one, before pause returns. So do the test
            // stream's frames.
            if (!Pumping && ChildrenOut == 0 && KtxOut == 0)
            {
                break;
            }
            NdisMSleep(100);
        }
    }

    Status = NDIS_STATUS_SUCCESS;

    pFilter->State = FilterPaused;

    DEBUGP(DL_TRACE, "<===FilterPause:  Status %x\n", Status);
    return Status;
}

_Use_decl_annotations_
NDIS_STATUS
FilterRestart(
    NDIS_HANDLE                     FilterModuleContext,
    PNDIS_FILTER_RESTART_PARAMETERS RestartParameters
    )
/*++

Routine Description:

    Filter restart routine.
    Start the datapath - begin sending and receiving NBLs.

Arguments:

    FilterModuleContext - pointer to the filter context stucture.
    RestartParameters   - additional information about the restart operation.

Return Value:

    NDIS_STATUS_SUCCESS: if filter restarts successfully
    NDIS_STATUS_XXX: Otherwise.

--*/
{
    NDIS_STATUS     Status;
    PMS_FILTER      pFilter = (PMS_FILTER)FilterModuleContext;
    NDIS_HANDLE     ConfigurationHandle = NULL;


    PNDIS_RESTART_GENERAL_ATTRIBUTES NdisGeneralAttributes;
    PNDIS_RESTART_ATTRIBUTES         NdisRestartAttributes;
    NDIS_CONFIGURATION_OBJECT        ConfigObject;

    DEBUGP(DL_TRACE, "===>FilterRestart:   FilterModuleContext %p\n", FilterModuleContext);

    FILTER_ASSERT(pFilter->State == FilterPaused);

    ConfigObject.Header.Type = NDIS_OBJECT_TYPE_CONFIGURATION_OBJECT;
    ConfigObject.Header.Revision = NDIS_CONFIGURATION_OBJECT_REVISION_1;
    ConfigObject.Header.Size = sizeof(NDIS_CONFIGURATION_OBJECT);
    ConfigObject.NdisHandle = FilterDriverHandle;
    ConfigObject.Flags = 0;

    Status = NdisOpenConfigurationEx(&ConfigObject, &ConfigurationHandle);
    if (Status != NDIS_STATUS_SUCCESS)
    {
        //
        // Filter driver can choose to fail the restart if it cannot open the configuration
        //

#if 0
        //
        // The code is here just to demonstrate how to call NDIS to write an
        // event to the eventlog.
        //
        PWCHAR              ErrorString = L"Ndislwf";

        DEBUGP(DL_WARN, "FilterRestart: Cannot open configuration.\n");
        NdisWriteEventLogEntry(FilterDriverObject,
                                EVENT_NDIS_DRIVER_FAILURE,
                                0,
                                1,
                                &ErrorString,
                                sizeof(Status),
                                &Status);
#endif

    }

    //
    // This sample doesn't actually do anything with the configuration handle;
    // it is opened here for illustrative purposes.  If you do not need to
    // read configuration, you may omit the code manipulating the
    // ConfigurationHandle.
    //

    if (Status == NDIS_STATUS_SUCCESS)
    {
        NdisCloseConfiguration(ConfigurationHandle);
    }

    NdisRestartAttributes = RestartParameters->RestartAttributes;

    //
    // If NdisRestartAttributes is not NULL, then the filter can modify generic
    // attributes and add new media specific info attributes at the end.
    // Otherwise, if NdisRestartAttributes is NULL, the filter should not try to
    // modify/add attributes.
    //
    if (NdisRestartAttributes != NULL)
    {
        PNDIS_RESTART_ATTRIBUTES   NextAttributes;

        ASSERT(NdisRestartAttributes->Oid == OID_GEN_MINIPORT_RESTART_ATTRIBUTES);

        NdisGeneralAttributes = (PNDIS_RESTART_GENERAL_ATTRIBUTES)NdisRestartAttributes->Data;

        //
        // Check to see if we need to change any attributes. For example, the
        // driver can change the current MAC address here. Or the driver can add
        // media specific info attributes.
        //
        NdisGeneralAttributes->LookaheadSize = 128;

        //
        // Check each attribute to see whether the filter needs to modify it.
        //
        NextAttributes = NdisRestartAttributes->Next;

        while (NextAttributes != NULL)
        {
            //
            // If somehow the filter needs to change a attributes which requires more space then
            // the current attributes:
            // 1. Remove the attribute from the Attributes list:
            //    TempAttributes = NextAttributes;
            //    NextAttributes = NextAttributes->Next;
            // 2. Free the memory for the current attributes: NdisFreeMemory(TempAttributes, 0 , 0);
            // 3. Dynamically allocate the memory for the new attributes by calling
            //    NdisAllocateMemoryWithTagPriority:
            //    NewAttributes = NdisAllocateMemoryWithTagPriority(Handle, size, Priority);
            // 4. Fill in the new attribute
            // 5. NewAttributes->Next = NextAttributes;
            // 6. NextAttributes = NewAttributes; // Just to make the next statement work.
            //
            NextAttributes = NextAttributes->Next;
        }

        //
        // Add a new attributes at the end
        // 1. Dynamically allocate the memory for the new attributes by calling
        //    NdisAllocateMemoryWithTagPriority.
        // 2. Fill in the new attribute
        // 3. NextAttributes->Next = NewAttributes;
        // 4. NewAttributes->Next = NULL;



    }

    //
    // If everything is OK, set the filter in running state.
    //
    pFilter->State = FilterRunning; // when successful


    Status = NDIS_STATUS_SUCCESS;


    //
    // Ensure the state is Paused if restart failed.
    //

    if (Status != NDIS_STATUS_SUCCESS)
    {
        pFilter->State = FilterPaused;
    }


    DEBUGP(DL_TRACE, "<===FilterRestart:  FilterModuleContext %p, Status %x\n", FilterModuleContext, Status);
    return Status;
}


_Use_decl_annotations_
VOID
FilterDetach(
    NDIS_HANDLE     FilterModuleContext
    )
/*++

Routine Description:

    Filter detach routine.
    This is a required function that will deallocate all the resources allocated during
    FilterAttach. NDIS calls FilterAttach to remove a filter instance from a filter stack.

Arguments:

    FilterModuleContext - pointer to the filter context area.

Return Value:
    None.

NOTE: Called at PASSIVE_LEVEL and the filter is in paused state

--*/
{
    PMS_FILTER                  pFilter = (PMS_FILTER)FilterModuleContext;
    BOOLEAN                      bFalse = FALSE;


    DEBUGP(DL_TRACE, "===>FilterDetach:    FilterInstance %p\n", FilterModuleContext);


    //
    // Filter must be in paused state
    //
    FILTER_ASSERT(pFilter->State == FilterPaused);


    //
    // Detach must not fail, so do not put any code here that can possibly fail.
    //

    //
    // Free filter instance name if allocated.
    //
    if (pFilter->FilterName.Buffer != NULL)
    {
        FILTER_FREE_MEM(pFilter->FilterName.Buffer);
    }


    // Off the list first, so no IOCTL (they all find the filter through the
    // list under FilterListLock) reaches its timers or slots while they are
    // torn down below.
    FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
    RemoveEntryList(&pFilter->FilterModuleLink);
    FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
    // A TX_SEND that found the filter before the unlink is still sending:
    // its slots and pool stay until it returns.
    while (InterlockedCompareExchange(&pFilter->TxRefs, 0, 0) != 0)
    {
        NdisMSleep(100);
    }

    // The timer goes first, at PASSIVE: deleting it waits for a running
    // callback, which is not allowed under a spin lock.
    if (pFilter->PaceTimer != NULL)
    {
        ExDeleteTimer(pFilter->PaceTimer, TRUE, TRUE, NULL);
        pFilter->PaceTimer = NULL;
    }
    // The test stream's timer, frames (all completed: pause waited) and
    // slots.
    ktxTeardown(pFilter);
    // Every child has completed (pause waited for them).
    if (pFilter->PaceNblPool != NULL)
    {
        NdisFreeNetBufferListPool(pFilter->PaceNblPool);
        pFilter->PaceNblPool = NULL;
    }
    if (pFilter->PaceRing != NULL)
    {
        NdisFreeMemory(pFilter->PaceRing, 0, 0);
        pFilter->PaceRing = NULL;
    }


    //
    // Free the memory allocated
    FILTER_FREE_MEM(pFilter);

    DEBUGP(DL_TRACE, "<===FilterDetach Successfully\n");
    return;
}

_Use_decl_annotations_
VOID
FilterUnload(
    PDRIVER_OBJECT      DriverObject
    )
/*++

Routine Description:

    Filter driver's unload routine.
    Deregister the driver from NDIS.

Arguments:

    DriverObject - pointer to the system's driver object structure
                   for this driver

Return Value:

    NONE

--*/
{
#if DBG
    BOOLEAN               bFalse = FALSE;
#endif

    UNREFERENCED_PARAMETER(DriverObject);

    DEBUGP(DL_TRACE, "===>FilterUnload\n");

    //
    // Should free the filter context list
    //
    FilterDeregisterDevice();
    NdisFDeregisterFilterDriver(FilterDriverHandle);

#if DBG
    FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
    ASSERT(IsListEmpty(&FilterModuleList));

    FILTER_RELEASE_LOCK(&FilterListLock, bFalse);

#endif

    FILTER_FREE_LOCK(&FilterListLock);

    DEBUGP(DL_TRACE, "<===FilterUnload\n");

    return;

}

_Use_decl_annotations_
NDIS_STATUS
FilterOidRequest(
    NDIS_HANDLE         FilterModuleContext,
    PNDIS_OID_REQUEST   Request
    )
/*++

Routine Description:

    Request handler
    Handle requests from upper layers

Arguments:

    FilterModuleContext   - our filter
    Request               - the request passed down


Return Value:

     NDIS_STATUS_SUCCESS
     NDIS_STATUS_PENDING
     NDIS_STATUS_XXX

NOTE: Called at <= DISPATCH_LEVEL  (unlike a miniport's MiniportOidRequest)

--*/
{
    PMS_FILTER              pFilter = (PMS_FILTER)FilterModuleContext;
    NDIS_STATUS             Status;
    PNDIS_OID_REQUEST       ClonedRequest=NULL;
    BOOLEAN                 bSubmitted = FALSE;
    PFILTER_REQUEST_CONTEXT Context;
    BOOLEAN                 bFalse = FALSE;


    DEBUGP(DL_TRACE, "===>FilterOidRequest: Request %p.\n", Request);

    //
    // Most of the time, a filter will clone the OID request and pass down
    // the clone.  When the clone completes, the filter completes the original
    // OID request.
    //
    // If your filter needs to modify a specific request, it can modify the
    // request before or after sending down the cloned request.  Or, it can
    // complete the original request on its own without sending down any
    // clone at all.
    //
    // If your filter driver does not need to modify any OID requests, then
    // you may simply omit this routine entirely; NDIS will pass OID requests
    // down on your behalf.  This is more efficient than implementing a
    // routine that does nothing but clone all requests, as in the sample here.
    //

    do
    {
        Status = NdisAllocateCloneOidRequest(pFilter->FilterHandle,
                                            Request,
                                            FILTER_TAG,
                                            &ClonedRequest);
        if (Status != NDIS_STATUS_SUCCESS)
        {
            DEBUGP(DL_WARN, "FilerOidRequest: Cannot Clone Request\n");
            break;
        }

        Context = (PFILTER_REQUEST_CONTEXT)(&ClonedRequest->SourceReserved[0]);
        *Context = Request;

        bSubmitted = TRUE;

        //
        // Use same request ID
        //
        ClonedRequest->RequestId = Request->RequestId;

        pFilter->PendingOidRequest = ClonedRequest;


        Status = NdisFOidRequest(pFilter->FilterHandle, ClonedRequest);

        if (Status != NDIS_STATUS_PENDING)
        {


            FilterOidRequestComplete(pFilter, ClonedRequest, Status);
            Status = NDIS_STATUS_PENDING;
        }


    }while (bFalse);

    if (bSubmitted == FALSE)
    {
        switch(Request->RequestType)
        {
            case NdisRequestMethod:
                Request->DATA.METHOD_INFORMATION.BytesRead = 0;
                Request->DATA.METHOD_INFORMATION.BytesNeeded = 0;
                Request->DATA.METHOD_INFORMATION.BytesWritten = 0;
                break;

            case NdisRequestSetInformation:
                Request->DATA.SET_INFORMATION.BytesRead = 0;
                Request->DATA.SET_INFORMATION.BytesNeeded = 0;
                break;

            case NdisRequestQueryInformation:
            case NdisRequestQueryStatistics:
            default:
                Request->DATA.QUERY_INFORMATION.BytesWritten = 0;
                Request->DATA.QUERY_INFORMATION.BytesNeeded = 0;
                break;
        }

    }
    DEBUGP(DL_TRACE, "<===FilterOidRequest: Status %8x.\n", Status);

    return Status;

}

_Use_decl_annotations_
VOID
FilterCancelOidRequest(
    NDIS_HANDLE             FilterModuleContext,
    PVOID                   RequestId
    )
/*++

Routine Description:

    Cancels an OID request

    If your filter driver does not intercept and hold onto any OID requests,
    then you do not need to implement this routine.  You may simply omit it.
    Furthermore, if the filter only holds onto OID requests so it can pass
    down a clone (the most common case) the filter does not need to implement
    this routine; NDIS will then automatically request that the lower-level
    filter/miniport cancel your cloned OID.

    Most filters do not need to implement this routine.

Arguments:

    FilterModuleContext   - our filter
    RequestId             - identifies the request(s) to cancel

--*/
{
    PMS_FILTER                          pFilter = (PMS_FILTER)FilterModuleContext;
    PNDIS_OID_REQUEST                   Request = NULL;
    PFILTER_REQUEST_CONTEXT             Context;
    PNDIS_OID_REQUEST                   OriginalRequest = NULL;
    BOOLEAN                             bFalse = FALSE;

    FILTER_ACQUIRE_LOCK(&pFilter->Lock, bFalse);

    Request = pFilter->PendingOidRequest;

    if (Request != NULL)
    {
        Context = (PFILTER_REQUEST_CONTEXT)(&Request->SourceReserved[0]);

        OriginalRequest = (*Context);
    }

    if ((OriginalRequest != NULL) && (OriginalRequest->RequestId == RequestId))
    {
        FILTER_RELEASE_LOCK(&pFilter->Lock, bFalse);

        NdisFCancelOidRequest(pFilter->FilterHandle, RequestId);
    }
    else
    {
        FILTER_RELEASE_LOCK(&pFilter->Lock, bFalse);
    }


}

_Use_decl_annotations_
VOID
FilterOidRequestComplete(
    NDIS_HANDLE         FilterModuleContext,
    PNDIS_OID_REQUEST   Request,
    NDIS_STATUS         Status
    )
/*++

Routine Description:

    Notification that an OID request has been completed

    If this filter sends a request down to a lower layer, and the request is
    pended, the FilterOidRequestComplete routine is invoked when the request
    is complete.  Most requests we've sent are simply clones of requests
    received from a higher layer; all we need to do is complete the original
    higher request.

    However, if this filter driver sends original requests down, it must not
    attempt to complete a pending request to the higher layer.

Arguments:

    FilterModuleContext   - our filter context area
    NdisRequest           - the completed request
    Status                - completion status

--*/
{
    PMS_FILTER                          pFilter = (PMS_FILTER)FilterModuleContext;
    PNDIS_OID_REQUEST                   OriginalRequest;
    PFILTER_REQUEST_CONTEXT             Context;
    BOOLEAN                             bFalse = FALSE;

    DEBUGP(DL_TRACE, "===>FilterOidRequestComplete, Request %p.\n", Request);

    Context = (PFILTER_REQUEST_CONTEXT)(&Request->SourceReserved[0]);
    OriginalRequest = (*Context);

    //
    // This is an internal request
    //
    if (OriginalRequest == NULL)
    {
        filterInternalRequestComplete(pFilter, Request, Status);
        return;
    }


    FILTER_ACQUIRE_LOCK(&pFilter->Lock, bFalse);

    ASSERT(pFilter->PendingOidRequest == Request);
    pFilter->PendingOidRequest = NULL;

    FILTER_RELEASE_LOCK(&pFilter->Lock, bFalse);


    //
    // Copy the information from the returned request to the original request
    //
    switch(Request->RequestType)
    {
        case NdisRequestMethod:
            OriginalRequest->DATA.METHOD_INFORMATION.OutputBufferLength =  Request->DATA.METHOD_INFORMATION.OutputBufferLength;
            OriginalRequest->DATA.METHOD_INFORMATION.BytesRead = Request->DATA.METHOD_INFORMATION.BytesRead;
            OriginalRequest->DATA.METHOD_INFORMATION.BytesNeeded = Request->DATA.METHOD_INFORMATION.BytesNeeded;
            OriginalRequest->DATA.METHOD_INFORMATION.BytesWritten = Request->DATA.METHOD_INFORMATION.BytesWritten;
            break;

        case NdisRequestSetInformation:
            OriginalRequest->DATA.SET_INFORMATION.BytesRead = Request->DATA.SET_INFORMATION.BytesRead;
            OriginalRequest->DATA.SET_INFORMATION.BytesNeeded = Request->DATA.SET_INFORMATION.BytesNeeded;
            break;

        case NdisRequestQueryInformation:
        case NdisRequestQueryStatistics:
        default:
            OriginalRequest->DATA.QUERY_INFORMATION.BytesWritten = Request->DATA.QUERY_INFORMATION.BytesWritten;
            OriginalRequest->DATA.QUERY_INFORMATION.BytesNeeded = Request->DATA.QUERY_INFORMATION.BytesNeeded;
            break;
    }


    (*Context) = NULL;

    NdisFreeCloneOidRequest(pFilter->FilterHandle, Request);

    NdisFOidRequestComplete(pFilter->FilterHandle, OriginalRequest, Status);

    DEBUGP(DL_TRACE, "<===FilterOidRequestComplete.\n");
}


_Use_decl_annotations_
VOID
FilterStatus(
    NDIS_HANDLE             FilterModuleContext,
    PNDIS_STATUS_INDICATION StatusIndication
    )
/*++

Routine Description:

    Status indication handler

Arguments:

    FilterModuleContext     - our filter context
    StatusIndication        - the status being indicated

NOTE: called at <= DISPATCH_LEVEL

  FILTER driver may call NdisFIndicateStatus to generate a status indication to
  all higher layer modules.

--*/
{
    PMS_FILTER              pFilter = (PMS_FILTER)FilterModuleContext;
#if DBG
    BOOLEAN                  bFalse = FALSE;
#endif

    DEBUGP(DL_TRACE, "===>FilterStatus, IndicateStatus = %8x.\n", StatusIndication->StatusCode);


    //
    // The filter may do processing on the status indication here, including
    // intercepting and dropping it entirely.  However, the sample does nothing
    // with status indications except pass them up to the higher layer.  It is
    // more efficient to omit the FilterStatus handler entirely if it does
    // nothing, but it is included in this sample for illustrative purposes.
    //

#if DBG
    FILTER_ACQUIRE_LOCK(&pFilter->Lock, bFalse);
    ASSERT(pFilter->bIndicating == FALSE);
    pFilter->bIndicating = TRUE;
    FILTER_RELEASE_LOCK(&pFilter->Lock, bFalse);
#endif // DBG

    //
    // dasio send pacer: follow the link speed.
    //
    if (StatusIndication->StatusCode == NDIS_STATUS_LINK_STATE &&
        StatusIndication->StatusBufferSize >= sizeof(NDIS_LINK_STATE))
    {
        PNDIS_LINK_STATE LinkState = (PNDIS_LINK_STATE)StatusIndication->StatusBuffer;
        BOOLEAN PassiveLevel = FALSE;
        FILTER_ACQUIRE_LOCK(&pFilter->Lock, PassiveLevel);
        pFilter->PaceLinkLast = LinkState->XmitLinkSpeed;
        pFilter->PaceLinkUpdates++;
        pacerApplyRateLocked(pFilter);
        FILTER_RELEASE_LOCK(&pFilter->Lock, PassiveLevel);
    }

    NdisFIndicateStatus(pFilter->FilterHandle, StatusIndication);

#if DBG
    FILTER_ACQUIRE_LOCK(&pFilter->Lock, bFalse);
    ASSERT(pFilter->bIndicating == TRUE);
    pFilter->bIndicating = FALSE;
    FILTER_RELEASE_LOCK(&pFilter->Lock, bFalse);
#endif // DBG

    DEBUGP(DL_TRACE, "<===FilterStatus.\n");

}

_Use_decl_annotations_
VOID
FilterDevicePnPEventNotify(
    NDIS_HANDLE             FilterModuleContext,
    PNET_DEVICE_PNP_EVENT   NetDevicePnPEvent
    )
/*++

Routine Description:

    Device PNP event handler

Arguments:

    FilterModuleContext         - our filter context
    NetDevicePnPEvent           - a Device PnP event

NOTE: called at PASSIVE_LEVEL

--*/
{
    PMS_FILTER             pFilter = (PMS_FILTER)FilterModuleContext;
    NDIS_DEVICE_PNP_EVENT  DevicePnPEvent = NetDevicePnPEvent->DevicePnPEvent;
#if DBG
    BOOLEAN                bFalse = FALSE;
#endif

    DEBUGP(DL_TRACE, "===>FilterDevicePnPEventNotify: NetPnPEvent = %p.\n", NetDevicePnPEvent);

    //
    // The filter may do processing on the event here, including intercepting
    // and dropping it entirely.  However, the sample does nothing with Device
    // PNP events, except pass them down to the next lower* layer.  It is more
    // efficient to omit the FilterDevicePnPEventNotify handler entirely if it
    // does nothing, but it is included in this sample for illustrative purposes.
    //
    // * Trivia: Device PNP events percolate DOWN the stack, instead of upwards
    // like status indications and Net PNP events.  So the next layer is the
    // LOWER layer.
    //

    switch (DevicePnPEvent)
    {

        case NdisDevicePnPEventQueryRemoved:
        case NdisDevicePnPEventRemoved:
        case NdisDevicePnPEventSurpriseRemoved:
        case NdisDevicePnPEventQueryStopped:
        case NdisDevicePnPEventStopped:
        case NdisDevicePnPEventPowerProfileChanged:
        case NdisDevicePnPEventFilterListChanged:

            break;

        default:
            DEBUGP(DL_ERROR, "FilterDevicePnPEventNotify: Invalid event.\n");
            FILTER_ASSERT(bFalse);

            break;
    }

    NdisFDevicePnPEventNotify(pFilter->FilterHandle, NetDevicePnPEvent);

    DEBUGP(DL_TRACE, "<===FilterDevicePnPEventNotify\n");

}

_Use_decl_annotations_
NDIS_STATUS
FilterNetPnPEvent(
    NDIS_HANDLE              FilterModuleContext,
    PNET_PNP_EVENT_NOTIFICATION NetPnPEventNotification
    )
/*++

Routine Description:

    Net PNP event handler

Arguments:

    FilterModuleContext         - our filter context
    NetPnPEventNotification     - a Net PnP event

NOTE: called at PASSIVE_LEVEL

--*/
{
    PMS_FILTER                pFilter = (PMS_FILTER)FilterModuleContext;
    NDIS_STATUS               Status = NDIS_STATUS_SUCCESS;

    //
    // The filter may do processing on the event here, including intercepting
    // and dropping it entirely.  However, the sample does nothing with Net PNP
    // events, except pass them up to the next higher layer.  It is more
    // efficient to omit the FilterNetPnPEvent handler entirely if it does
    // nothing, but it is included in this sample for illustrative purposes.
    //

    Status = NdisFNetPnPEvent(pFilter->FilterHandle, NetPnPEventNotification);

    return Status;
}


// ---- dasio send pacer -------------------------------------------------

// Bytes of an NBL: the sum of its NET_BUFFERs' data.
static ULONG
pacerNblBytes(
    _In_ PNET_BUFFER_LIST Nbl
    )
{
    ULONG Bytes = 0;
    PNET_BUFFER Nb;
    for (Nb = NET_BUFFER_LIST_FIRST_NB(Nbl); Nb != NULL; Nb = NET_BUFFER_NEXT_NB(Nb))
    {
        Bytes += NET_BUFFER_DATA_LENGTH(Nb);
    }
    return Bytes;
}

// The class of an NBL from its first NET_BUFFER's headers. A header that
// cannot be read makes it "other", the safe side (it is only paced).
static enum pacer_class
pacerNblClass(
    _In_ PNET_BUFFER_LIST Nbl
    )
{
    UCHAR Storage[PACER_HEADER_BYTES];
    PNET_BUFFER Nb = NET_BUFFER_LIST_FIRST_NB(Nbl);
    ULONG Length;
    PUCHAR Header;

    if (Nb == NULL)
    {
        return PACER_OTHER;
    }
    Length = NET_BUFFER_DATA_LENGTH(Nb);
    if (Length > PACER_HEADER_BYTES)
    {
        Length = PACER_HEADER_BYTES;
    }
    if (Length == 0)
    {
        return PACER_OTHER;
    }
    Header = (PUCHAR)NdisGetDataBuffer(Nb, Length, Storage, 1, 0);
    if (Header == NULL)
    {
        return PACER_OTHER;
    }
    return pacer_classify(Header, Length);
}

// Whether a received NBL waits for the deferred-receive DPC (its first
// packet's headers, pacer_rx_defer). Unreadable: not deferred.
static int
pacerNblRxDefer(
    _In_ PNET_BUFFER_LIST Nbl
    )
{
    UCHAR Storage[PACER_HEADER_BYTES];
    PNET_BUFFER Nb = NET_BUFFER_LIST_FIRST_NB(Nbl);
    ULONG Length;
    PUCHAR Header;

    if (Nb == NULL)
    {
        return 0;
    }
    Length = NET_BUFFER_DATA_LENGTH(Nb);
    if (Length > PACER_HEADER_BYTES)
    {
        Length = PACER_HEADER_BYTES;
    }
    if (Length == 0)
    {
        return 0;
    }
    Header = (PUCHAR)NdisGetDataBuffer(Nb, Length, Storage, 1, 0);
    if (Header == NULL)
    {
        return 0;
    }
    return pacer_rx_defer(Header, Length);
}

// Now, in 100 ns units (the interrupt time, precise).
static ULONG64
pacerNow(
    VOID
    )
{
    ULONG64 Qpc;
    return KeQueryInterruptTimePrecise(&Qpc);
}

// The share of the link and the burst other traffic gets: set live by
// IOCTL_DASIO_PACER_SET for all bound adapters (0 % = no pacing, burst 0 =
// one timer tick). Written under FilterListLock and each filter's Lock.
ULONG PacerRatePercent = PACER_RATE_PERCENT;
ULONG PacerBurstBytes = PACER_DEFAULT_BURST;

// Rate (bytes/s) and burst for a link speed (bits/s), from the settings
// above. An unknown or zero speed means no pacing.
static VOID
pacerRateFor(
    _In_ ULONG64 LinkBitsPerSecond,
    _Out_ ULONG64 *Rate,
    _Out_ ULONG *Burst
    )
{
    uint64_t R;
    uint32_t B;
    pacer_rate_for(LinkBitsPerSecond, PacerRatePercent, PacerBurstBytes, PACER_TICK_100NS, PACER_MIN_BURST, &R, &B);
    *Rate = R;
    *Burst = B;
}

// Under Lock: the rate and burst for the link speed last reported and the
// current settings; tokens above the new burst are dropped.
VOID
pacerApplyRateLocked(
    _In_ PMS_FILTER Filter
    )
{
    ULONG64 Rate;
    ULONG Burst;
    if (Filter->PaceTimer == NULL || Filter->PaceRing == NULL)
    {
        return;
    }
    pacerRateFor(Filter->PaceLinkLast, &Rate, &Burst);
    Filter->Pacer.rate = Rate;
    Filter->Pacer.burst = Burst;
    if (Filter->Pacer.tokens > (LONG64)Burst)
    {
        Filter->Pacer.tokens = Burst;
    }
}

// Under Lock: arms the timer for the ring's head unless it is armed or
// nothing waits.
static VOID
pacerArmLocked(
    _In_ PMS_FILTER Filter,
    _In_ ULONG64 Now
    )
{
    ULONG64 Wait;
    if (Filter->PaceTimerArmed || Filter->PaceRingCount == 0 || Filter->PaceTimer == NULL)
    {
        return;
    }
    Wait = pacer_wait(&Filter->Pacer, Now);
    if (Wait == 0)
    {
        Wait = 1;
    }
    Filter->PaceTimerArmed = TRUE;
    Filter->PaceTimerArms++;
    ExSetTimer(Filter->PaceTimer, -(LONG64)Wait, 0, NULL);
}

// The pump (pacer_pump in the core) around this filter: its lock, its
// ring, its timer and NdisFSendNetBufferLists. One runs at a time; a send
// the miniport completes inline calls it again and that call returns at
// once (dasio#1: the HIL 2 / 3c double faults).
typedef struct _PACER_PUMP_CTX
{
    PMS_FILTER  Filter;
    BOOLEAN     DispatchLevel;
} PACER_PUMP_CTX, *PPACER_PUMP_CTX;

static VOID
pacerPumpLock(
    _In_ PVOID Ctx
    )
{
    PPACER_PUMP_CTX C = (PPACER_PUMP_CTX)Ctx;
    FILTER_ACQUIRE_LOCK(&C->Filter->Lock, C->DispatchLevel);
}

static VOID
pacerPumpUnlock(
    _In_ PVOID Ctx
    )
{
    PPACER_PUMP_CTX C = (PPACER_PUMP_CTX)Ctx;
    FILTER_RELEASE_LOCK(&C->Filter->Lock, C->DispatchLevel);
}

// Under Lock: the due NBLs off the ring's head, in order. Nothing while the
// filter is not running (pause completes the ring itself).
static uint32_t
pacerPumpTake(
    _In_ PVOID Ctx,
    _Out_writes_to_(Max, return) struct pacer_taken *Out,
    _In_ uint32_t Max
    )
{
    PMS_FILTER Filter = ((PPACER_PUMP_CTX)Ctx)->Filter;
    ULONG64 Now = pacerNow();
    uint32_t Count = 0;

    Filter->PacePumps++;
    if (Filter->State != FilterRunning)
    {
        return 0;
    }
    while (Count < Max && Filter->PaceRingCount > 0 &&
           pacer_take(&Filter->Pacer, pacerNblBytes(Filter->PaceRing[Filter->PaceRingHead].Nbl), Now))
    {
        Out[Count].item = Filter->PaceRing[Filter->PaceRingHead].Nbl;
        Out[Count].flags = Filter->PaceRing[Filter->PaceRingHead].SendFlags;
        Filter->PaceRingHead = (Filter->PaceRingHead + 1) % PACER_RING_SIZE;
        Filter->PaceRingCount--;
        Count++;
    }
    Filter->PacePumpSent += Count;
    return Count;
}

// Under Lock: nothing is due; the timer covers what waits.
static VOID
pacerPumpIdle(
    _In_ PVOID Ctx
    )
{
    PMS_FILTER Filter = ((PPACER_PUMP_CTX)Ctx)->Filter;
    if (Filter->State == FilterRunning)
    {
        pacerArmLocked(Filter, pacerNow());
    }
}

// Outside Lock: one NBL goes down alone with the flags it came with.
static VOID
pacerPumpSend(
    _In_ PVOID Ctx,
    _In_ const struct pacer_taken *Taken
    )
{
    PPACER_PUMP_CTX C = (PPACER_PUMP_CTX)Ctx;
    ULONG Flags = Taken->flags & ~NDIS_SEND_FLAGS_DISPATCH_LEVEL;
    if (C->DispatchLevel)
    {
        Flags |= NDIS_SEND_FLAGS_DISPATCH_LEVEL;
    }
    NdisFSendNetBufferLists(C->Filter->FilterHandle, (PNET_BUFFER_LIST)Taken->item, NDIS_DEFAULT_PORT_NUMBER, Flags);
}

static const struct pacer_pump_ops PacerPumpOps =
{
    pacerPumpLock, pacerPumpUnlock, pacerPumpTake, pacerPumpIdle, pacerPumpSend
};

static VOID
pacerPump(
    _In_ PMS_FILTER Filter,
    _In_ BOOLEAN DispatchLevel,
    _In_ BOOLEAN FromTimer
    )
{
    PACER_PUMP_CTX Ctx;
    Ctx.Filter = Filter;
    Ctx.DispatchLevel = DispatchLevel;
    if (FromTimer)
    {
        FILTER_ACQUIRE_LOCK(&Filter->Lock, DispatchLevel);
        Filter->PaceTimerArmed = FALSE;
        Filter->PaceTimerFires++;
        FILTER_RELEASE_LOCK(&Filter->Lock, DispatchLevel);
    }
    pacer_pump(&Filter->PacePumping, &PacerPumpOps, &Ctx);
}

// The timer: sends what the tokens now allow and re-arms for the rest.
_Use_decl_annotations_
static VOID
pacerTimer(
    PEX_TIMER Timer,
    PVOID Context
    )
{
    UNREFERENCED_PARAMETER(Timer);
    pacerPump((PMS_FILTER)Context, TRUE, TRUE);
}

// One child completed: freed; its parent goes onto *UpTail when it was the
// last child.
static VOID
pacerChildDone(
    _In_ PMS_FILTER Filter,
    _In_ PNET_BUFFER_LIST Child,
    _In_ BOOLEAN DispatchLevel,
    _Inout_ PNET_BUFFER_LIST **UpTail
    )
{
    PPACER_SPLIT_REC Rec = *(PPACER_SPLIT_REC *)NET_BUFFER_LIST_CONTEXT_DATA_START(Child);
    NDIS_STATUS Status = NET_BUFFER_LIST_STATUS(Child);
    int Last;

    NdisFreeNetBufferList(Child);
    FILTER_ACQUIRE_LOCK(&Filter->Lock, DispatchLevel);
    Last = pacer_split_done(&Rec->Split, Status);
    if (Filter->PaceChildrenOut > 0)
    {
        Filter->PaceChildrenOut--;
    }
    FILTER_RELEASE_LOCK(&Filter->Lock, DispatchLevel);
    if (Last)
    {
        PNET_BUFFER_LIST Parent = Rec->Parent;
        NET_BUFFER_LIST_STATUS(Parent) = Rec->Split.status;
        NET_BUFFER_LIST_NEXT_NBL(Parent) = NULL;
        **UpTail = Parent;
        *UpTail = &NET_BUFFER_LIST_NEXT_NBL(Parent);
        NdisFreeMemory(Rec, 0, 0);
    }
}

// Completes a chain upward: our child NBLs complete into their parents,
// the others go up as they are. SetStatus: they never went down (pause,
// cancel), with Status.
static VOID
pacerComplete(
    _In_ PMS_FILTER Filter,
    _In_opt_ PNET_BUFFER_LIST Nbls,
    _In_ BOOLEAN SetStatus,
    _In_ NDIS_STATUS Status,
    _In_ BOOLEAN DispatchLevel,
    _In_ ULONG UpFlags
    )
{
    PNET_BUFFER_LIST Up = NULL;
    PNET_BUFFER_LIST *UpTail = &Up;
    PNET_BUFFER_LIST Nbl = Nbls;

    while (Nbl != NULL)
    {
        PNET_BUFFER_LIST Next = NET_BUFFER_LIST_NEXT_NBL(Nbl);
        NET_BUFFER_LIST_NEXT_NBL(Nbl) = NULL;
        if (SetStatus)
        {
            NET_BUFFER_LIST_STATUS(Nbl) = Status;
        }
        if (Filter->PaceNblPool != NULL && NdisGetPoolFromNetBufferList(Nbl) == Filter->PaceNblPool)
        {
            pacerChildDone(Filter, Nbl, DispatchLevel, &UpTail);
        }
        else if (Filter->KtxPool != NULL && NdisGetPoolFromNetBufferList(Nbl) == Filter->KtxPool)
        {
            // A test stream frame: ours, it never goes up.
            ktxDone(Filter, Nbl, DispatchLevel);
        }
        else
        {
            *UpTail = Nbl;
            UpTail = &NET_BUFFER_LIST_NEXT_NBL(Nbl);
        }
        Nbl = Next;
    }
    if (Up != NULL)
    {
        if (DispatchLevel)
        {
            UpFlags |= NDIS_SEND_COMPLETE_FLAGS_DISPATCH_LEVEL;
        }
        NdisFSendNetBufferListsComplete(Filter->FilterHandle, Up, UpFlags);
    }
}

// Splits an NBL of Nbs packets into child NBLs of one packet each, from
// the filter's pool, over the parent's buffers. NULL when it cannot (the
// NBL then goes as a whole).
static PNET_BUFFER_LIST
pacerSplit(
    _In_ PMS_FILTER Filter,
    _In_ PNET_BUFFER_LIST Nbl,
    _In_ ULONG Nbs,
    _In_ BOOLEAN DispatchLevel
    )
{
    PPACER_SPLIT_REC Rec = NULL;
    PNET_BUFFER_LIST Head = NULL;
    PNET_BUFFER_LIST *Tail = &Head;
    PNET_BUFFER Nb;
    BOOLEAN Failed = FALSE;

    if (Filter->PaceNblPool != NULL)
    {
        Rec = (PPACER_SPLIT_REC)NdisAllocateMemoryWithTagPriority(
            FilterDriverHandle, sizeof(PACER_SPLIT_REC), FILTER_ALLOC_TAG, NormalPoolPriority);
    }
    if (Rec == NULL)
    {
        Failed = TRUE;
    }
    else
    {
        Rec->Parent = Nbl;
        pacer_split_init(&Rec->Split, Nbs);
        for (Nb = NET_BUFFER_LIST_FIRST_NB(Nbl); Nb != NULL; Nb = NET_BUFFER_NEXT_NB(Nb))
        {
            PNET_BUFFER_LIST Child = NdisAllocateNetBufferAndNetBufferList(
                Filter->PaceNblPool, PACER_CHILD_CONTEXT, 0,
                NET_BUFFER_CURRENT_MDL(Nb), NET_BUFFER_CURRENT_MDL_OFFSET(Nb), NET_BUFFER_DATA_LENGTH(Nb));
            if (Child == NULL)
            {
                Failed = TRUE;
                break;
            }
            Child->SourceHandle = Filter->FilterHandle;
            Child->ParentNetBufferList = Nbl;
            NdisCopySendNetBufferListInfo(Child, Nbl);
            NDIS_SET_NET_BUFFER_LIST_CANCEL_ID(Child, NDIS_GET_NET_BUFFER_LIST_CANCEL_ID(Nbl));
            *(PPACER_SPLIT_REC *)NET_BUFFER_LIST_CONTEXT_DATA_START(Child) = Rec;
            *Tail = Child;
            Tail = &NET_BUFFER_LIST_NEXT_NBL(Child);
        }
        if (Failed)
        {
            while (Head != NULL)
            {
                PNET_BUFFER_LIST Next = NET_BUFFER_LIST_NEXT_NBL(Head);
                NdisFreeNetBufferList(Head);
                Head = Next;
            }
            NdisFreeMemory(Rec, 0, 0);
        }
    }

    FILTER_ACQUIRE_LOCK(&Filter->Lock, DispatchLevel);
    if (Failed)
    {
        Filter->PaceSplitFails++;
    }
    else
    {
        Filter->PaceSplits++;
        Filter->PaceSplitChildren += Nbs;
        Filter->PaceChildrenOut += Nbs;
    }
    FILTER_RELEASE_LOCK(&Filter->Lock, DispatchLevel);
    return Failed ? NULL : Head;
}

// One send of Bytes: TRUE when it goes down now, FALSE when it waits in
// the ring (in order, for the timer or a completion).
static BOOLEAN
pacerAdmit(
    _In_ PMS_FILTER Filter,
    _In_ PNET_BUFFER_LIST Nbl,
    _In_ ULONG Bytes,
    _In_ NDIS_PORT_NUMBER PortNumber,
    _In_ ULONG SendFlags,
    _In_ BOOLEAN DispatchLevel
    )
{
    BOOLEAN Down = TRUE;
    ULONG64 Now = pacerNow();
    FILTER_ACQUIRE_LOCK(&Filter->Lock, DispatchLevel);
    if (Filter->State == FilterRunning &&
        PortNumber == NDIS_DEFAULT_PORT_NUMBER &&
        Filter->PaceRing != NULL &&
        Filter->PaceRingCount < PACER_RING_SIZE &&
        !pacer_offer(&Filter->Pacer, Bytes, Now))
    {
        PPACER_ENTRY Entry = &Filter->PaceRing[(Filter->PaceRingHead + Filter->PaceRingCount) % PACER_RING_SIZE];
        Entry->Nbl = Nbl;
        Entry->SendFlags = SendFlags;
        Filter->PaceRingCount++;
        Down = FALSE;
        pacerArmLocked(Filter, Now);
    }
    FILTER_RELEASE_LOCK(&Filter->Lock, DispatchLevel);
    return Down;
}

_Use_decl_annotations_
VOID
FilterSendNetBufferListsComplete(
    NDIS_HANDLE         FilterModuleContext,
    PNET_BUFFER_LIST    NetBufferLists,
    ULONG               SendCompleteFlags
    )
/*++

Routine Description:

    The miniport finished NBLs this filter sent down: a chance to send
    what the tokens allow; then the NBLs complete upwards unchanged.

--*/
{
    PMS_FILTER         pFilter = (PMS_FILTER)FilterModuleContext;
    BOOLEAN            DispatchLevel = NDIS_TEST_SEND_COMPLETE_AT_DISPATCH_LEVEL(SendCompleteFlags);

    // A completion is a chance to send what the tokens allow by now.
    pacerPump(pFilter, DispatchLevel, FALSE);

    // Our child NBLs complete into their parents; the rest go up as is.
    pacerComplete(pFilter, NetBufferLists, FALSE, NDIS_STATUS_SUCCESS, DispatchLevel,
                  SendCompleteFlags & ~NDIS_SEND_COMPLETE_FLAGS_DISPATCH_LEVEL);
}

_Use_decl_annotations_
VOID
FilterSendNetBufferLists(
    NDIS_HANDLE         FilterModuleContext,
    PNET_BUFFER_LIST    NetBufferLists,
    NDIS_PORT_NUMBER    PortNumber,
    ULONG               SendFlags
    )
/*++

Routine Description:

    Priority NBLs go down at once. Other NBLs go down while the pacer has
    tokens (a rate below the link's, bursts of one timer tick); the rest
    wait here, in order, for the timer or a completion. While the filter
    is not running nothing is held back.

--*/
{
    PMS_FILTER          pFilter = (PMS_FILTER)FilterModuleContext;
    BOOLEAN             DispatchLevel = NDIS_TEST_SEND_AT_DISPATCH_LEVEL(SendFlags);
    PNET_BUFFER_LIST    CurrNbl = NetBufferLists;
    PNET_BUFFER_LIST    Down = NULL;
    PNET_BUFFER_LIST   *DownTail = &Down;

    // Paused or pausing: nothing goes down (as Microsoft's sample does).
    FILTER_ACQUIRE_LOCK(&pFilter->Lock, DispatchLevel);
    if (pFilter->State != FilterRunning)
    {
        FILTER_RELEASE_LOCK(&pFilter->Lock, DispatchLevel);
        pacerComplete(pFilter, NetBufferLists, TRUE, NDIS_STATUS_PAUSED, DispatchLevel, 0);
        return;
    }
    FILTER_RELEASE_LOCK(&pFilter->Lock, DispatchLevel);

    while (CurrNbl != NULL)
    {
        PNET_BUFFER_LIST Next = NET_BUFFER_LIST_NEXT_NBL(CurrNbl);
        BOOLEAN GoesDown = TRUE;
        NET_BUFFER_LIST_NEXT_NBL(CurrNbl) = NULL;

        if (pacerNblClass(CurrNbl) == PACER_PRIORITY)
        {
            FILTER_ACQUIRE_LOCK(&pFilter->Lock, DispatchLevel);
            pacer_note_priority(&pFilter->Pacer);
            FILTER_RELEASE_LOCK(&pFilter->Lock, DispatchLevel);
        }
        else
        {
            ULONG Bytes = pacerNblBytes(CurrNbl);
            ULONG Nbs = 0;
            PNET_BUFFER Nb;
            PNET_BUFFER_LIST Children = NULL;
            for (Nb = NET_BUFFER_LIST_FIRST_NB(CurrNbl); Nb != NULL; Nb = NET_BUFFER_NEXT_NB(Nb))
            {
                Nbs++;
            }
            FILTER_ACQUIRE_LOCK(&pFilter->Lock, DispatchLevel);
            // What tcpip hands down: a TCP burst comes as NBLs of up to
            // hundreds of packets (dasio#1), so they are split below.
            pFilter->PaceOtherNbls++;
            pFilter->PaceOtherNbs += Nbs;
            if (Nbs > pFilter->PaceMaxNbs)
            {
                pFilter->PaceMaxNbs = Nbs;
            }
            if (Bytes > pFilter->PaceMaxNblBytes)
            {
                pFilter->PaceMaxNblBytes = Bytes;
            }
            FILTER_RELEASE_LOCK(&pFilter->Lock, DispatchLevel);

            if (Nbs > 1 && PortNumber == NDIS_DEFAULT_PORT_NUMBER)
            {
                Children = pacerSplit(pFilter, CurrNbl, Nbs, DispatchLevel);
            }
            if (Children != NULL)
            {
                // Each packet is paced on its own; the parent completes
                // upward with its last child.
                GoesDown = FALSE;
                while (Children != NULL)
                {
                    PNET_BUFFER_LIST Child = Children;
                    Children = NET_BUFFER_LIST_NEXT_NBL(Child);
                    NET_BUFFER_LIST_NEXT_NBL(Child) = NULL;
                    if (pacerAdmit(pFilter, Child, NET_BUFFER_DATA_LENGTH(NET_BUFFER_LIST_FIRST_NB(Child)),
                                   PortNumber, SendFlags, DispatchLevel))
                    {
                        *DownTail = Child;
                        DownTail = &NET_BUFFER_LIST_NEXT_NBL(Child);
                    }
                }
            }
            else
            {
                GoesDown = pacerAdmit(pFilter, CurrNbl, Bytes, PortNumber, SendFlags, DispatchLevel);
            }
        }

        if (GoesDown)
        {
            *DownTail = CurrNbl;
            DownTail = &NET_BUFFER_LIST_NEXT_NBL(CurrNbl);
        }
        CurrNbl = Next;
    }

    if (Down != NULL)
    {
        NdisFSendNetBufferLists(pFilter->FilterHandle, Down, PortNumber, SendFlags);
    }
}

_Use_decl_annotations_
VOID
FilterReturnNetBufferLists(
    NDIS_HANDLE         FilterModuleContext,
    PNET_BUFFER_LIST    NetBufferLists,
    ULONG               ReturnFlags
    )
/*++

Routine Description:

    FilterReturnNetBufferLists handler.
    FilterReturnNetBufferLists is an optional function. If provided, NDIS calls
    FilterReturnNetBufferLists to return the ownership of one or more NetBufferLists
    and their embedded NetBuffers to the filter driver. If this handler is NULL, NDIS
    will skip calling this filter when returning NetBufferLists to the underlying
    miniport and will call the next lower driver in the stack. A filter that doesn't
    provide a FilterReturnNetBufferLists handler cannot originate a receive indication
    on its own.

Arguments:

    FilterInstanceContext       - our filter context area
    NetBufferLists              - a linked list of NetBufferLists that this
                                  filter driver indicated in a previous call to
                                  NdisFIndicateReceiveNetBufferLists
    ReturnFlags                 - flags specifying if the caller is at DISPATCH_LEVEL

--*/
{
    PMS_FILTER          pFilter = (PMS_FILTER)FilterModuleContext;
    PNET_BUFFER_LIST    CurrNbl = NetBufferLists;
    UINT                NumOfNetBufferLists = 0;
    BOOLEAN             DispatchLevel;
    ULONG               Ref;

    DEBUGP(DL_TRACE, "===>ReturnNetBufferLists, NetBufferLists is %p.\n", NetBufferLists);


    //
    // If your filter injected any receive packets into the datapath to be
    // received, you must identify their NBLs here and remove them from the
    // chain.  Do not attempt to receive-return your NBLs down to the lower
    // layer.
    //

    //
    // If your filter has modified any NBLs (or NBs, MDLs, etc) in your
    // FilterReceiveNetBufferLists handler, you must undo the modifications here.
    // In general, NBLs must be returned in the same condition in which you had
    // you received them.  (Exceptions: the NBLs can be re-ordered on the linked
    // list, and the scratch fields are don't-care).
    //

    if (pFilter->TrackReceives)
    {
        while (CurrNbl)
        {
            NumOfNetBufferLists ++;
            CurrNbl = NET_BUFFER_LIST_NEXT_NBL(CurrNbl);
        }
    }


    // Return the received NBLs.  If you removed any NBLs from the chain, make
    // sure the chain isn't empty (i.e., NetBufferLists!=NULL).

    NdisFReturnNetBufferLists(pFilter->FilterHandle, NetBufferLists, ReturnFlags);

    if (pFilter->TrackReceives)
    {
        DispatchLevel = NDIS_TEST_RETURN_AT_DISPATCH_LEVEL(ReturnFlags);
        FILTER_ACQUIRE_LOCK(&pFilter->Lock, DispatchLevel);

        pFilter->OutstandingRcvs -= NumOfNetBufferLists;
        Ref = pFilter->OutstandingRcvs;
        FILTER_LOG_RCV_REF(3, pFilter, NetBufferLists, Ref);
        FILTER_RELEASE_LOCK(&pFilter->Lock, DispatchLevel);
    }


    DEBUGP(DL_TRACE, "<===ReturnNetBufferLists.\n");


}


_Use_decl_annotations_
VOID
FilterReceiveNetBufferLists(
    NDIS_HANDLE         FilterModuleContext,
    PNET_BUFFER_LIST    NetBufferLists,
    NDIS_PORT_NUMBER    PortNumber,
    ULONG               NumberOfNetBufferLists,
    ULONG               ReceiveFlags
    )
/*++

Routine Description:

    FilerReceiveNetBufferLists is an optional function for filter drivers.
    If provided, this function processes receive indications made by underlying
    NIC or lower level filter drivers. This function  can also be called as a
    result of loopback. If this handler is NULL, NDIS will skip calling this
    filter when processing a receive indication and will call the next higher
    driver in the stack. A filter that doesn't provide a
    FilterReceiveNetBufferLists handler cannot provide a
    FilterReturnNetBufferLists handler and cannot a initiate an original receive
    indication on its own.

Arguments:

    FilterModuleContext      - our filter context area.
    NetBufferLists           - a linked list of NetBufferLists
    PortNumber               - Port on which the receive is indicated
    ReceiveFlags             -

N.B.: It is important to check the ReceiveFlags in NDIS_TEST_RECEIVE_CANNOT_PEND.
    This controls whether the receive indication is an synchronous or
    asynchronous function call.

--*/
{

    PMS_FILTER          pFilter = (PMS_FILTER)FilterModuleContext;
    BOOLEAN             DispatchLevel;
    ULONG               Ref;
    BOOLEAN             bFalse = FALSE;
#if DBG
    ULONG               ReturnFlags;
#endif

    DEBUGP(DL_TRACE, "===>ReceiveNetBufferList: NetBufferLists = %p.\n", NetBufferLists);
    do
    {

        DispatchLevel = NDIS_TEST_RECEIVE_AT_DISPATCH_LEVEL(ReceiveFlags);
#if DBG
        FILTER_ACQUIRE_LOCK(&pFilter->Lock, DispatchLevel);

        if (pFilter->State != FilterRunning)
        {
            FILTER_RELEASE_LOCK(&pFilter->Lock, DispatchLevel);

            if (NDIS_TEST_RECEIVE_CAN_PEND(ReceiveFlags))
            {
                ReturnFlags = 0;
                if (NDIS_TEST_RECEIVE_AT_DISPATCH_LEVEL(ReceiveFlags))
                {
                    NDIS_SET_RETURN_FLAG(ReturnFlags, NDIS_RETURN_FLAGS_DISPATCH_LEVEL);
                }

                NdisFReturnNetBufferLists(pFilter->FilterHandle, NetBufferLists, ReturnFlags);
            }
            break;
        }
        FILTER_RELEASE_LOCK(&pFilter->Lock, DispatchLevel);
#endif

        ASSERT(NumberOfNetBufferLists >= 1);

        //
        // If you would like to drop a received packet, then you must carefully
        // modify the NBL chain as follows:
        //
        //     if NDIS_TEST_RECEIVE_CANNOT_PEND(ReceiveFlags):
        //         For each NBL that is NOT dropped, temporarily unlink it from
        //         the linked list, and indicate it up alone with
        //         NdisFIndicateReceiveNetBufferLists and the
        //         NDIS_RECEIVE_FLAGS_RESOURCES flag set.  Then immediately
        //         relink the NBL back into the chain.  When all NBLs have been
        //         indicated up, you may return from this function.
        //     otherwise (NDIS_TEST_RECEIVE_CANNOT_PEND is FALSE):
        //         Divide the linked list of NBLs into two chains: one chain
        //         of packets to drop, and everything else in another chain.
        //         Return the first chain with NdisFReturnNetBufferLists, and
        //         indicate up the rest with NdisFIndicateReceiveNetBufferLists.
        //
        // Note: on the receive path for Ethernet packets, one NBL will have
        // exactly one NB.  So (assuming you are receiving on Ethernet, or are
        // attached above Native WiFi) you do not need to worry about dropping
        // one NB, but trying to indicate up the remaining NBs on the same NBL.
        // In other words, if the first NB should be dropped, drop the whole NBL.
        //

        //
        // If you would like to modify a packet, and can do so quickly, you may
        // do it here.  However, make sure you save enough information to undo
        // your modification in the FilterReturnNetBufferLists handler.
        //

        //
        // If necessary, queue the NetBufferLists in a local structure for later
        // processing.  However, do not queue them for "too long", or else the
        // system's performance may be degraded.  If you need to hold onto an
        // NBL for an unbounded amount of time, then allocate memory, perform a
        // deep copy, and return the original NBL.
        //

        if (pFilter->TrackReceives)
        {
            FILTER_ACQUIRE_LOCK(&pFilter->Lock, DispatchLevel);
            pFilter->OutstandingRcvs += NumberOfNetBufferLists;
            Ref = pFilter->OutstandingRcvs;

            FILTER_LOG_RCV_REF(1, pFilter, NetBufferLists, Ref);
            FILTER_RELEASE_LOCK(&pFilter->Lock, DispatchLevel);
        }

        // dasio deferred receive: TCP and discovery chatter (whose upper
        // work may be long: WFP's flow-hash on mDNS) wait for our DPC and
        // the NIC's DPC returns at once; every other packet, Dante audio of
        // any DSCP and PTP included, goes up at once (pacer_rx_defer). Not when they cannot pend (the
        // miniport needs them back now) or on another port.
        if (NDIS_TEST_RECEIVE_CAN_PEND(ReceiveFlags) && PortNumber == NDIS_DEFAULT_PORT_NUMBER &&
            pFilter->RxdEnabled)
        {
            PNET_BUFFER_LIST Prio = NULL;
            PNET_BUFFER_LIST *PrioTail = &Prio;
            PNET_BUFFER_LIST Other = NULL;
            PNET_BUFFER_LIST *OtherTail = &Other;
            ULONG PrioCount = 0;
            ULONG OtherCount = 0;
            PNET_BUFFER_LIST Nbl = NetBufferLists;
            while (Nbl != NULL)
            {
                PNET_BUFFER_LIST Next = NET_BUFFER_LIST_NEXT_NBL(Nbl);
                NET_BUFFER_LIST_NEXT_NBL(Nbl) = NULL;
                if (!pacerNblRxDefer(Nbl))
                {
                    *PrioTail = Nbl;
                    PrioTail = &NET_BUFFER_LIST_NEXT_NBL(Nbl);
                    PrioCount++;
                }
                else
                {
                    *OtherTail = Nbl;
                    OtherTail = &NET_BUFFER_LIST_NEXT_NBL(Nbl);
                    OtherCount++;
                }
                Nbl = Next;
            }
            FILTER_ACQUIRE_LOCK(&pFilter->Lock, DispatchLevel);
            if (pFilter->State != FilterRunning)
            {
                // Not running: everything goes up at once, in one chain.
                *PrioTail = Other;
                PrioCount += OtherCount;
                Other = NULL;
                OtherCount = 0;
            }
            if (Other != NULL)
            {
                *pFilter->RxdTail = Other;
                pFilter->RxdTail = OtherTail;
                pFilter->RxdQueued += OtherCount;
                pFilter->RxdDeferred += OtherCount;
            }
            pFilter->RxdPriority += PrioCount;
            FILTER_RELEASE_LOCK(&pFilter->Lock, DispatchLevel);
            if (Other != NULL)
            {
                KeInsertQueueDpc(&pFilter->RxdDpc, NULL, NULL);
            }
            if (Prio != NULL)
            {
                NdisFIndicateReceiveNetBufferLists(pFilter->FilterHandle, Prio, PortNumber, PrioCount, ReceiveFlags);
            }
            break;
        }
        else
        {
            FILTER_ACQUIRE_LOCK(&pFilter->Lock, DispatchLevel);
            pFilter->RxdInline += NumberOfNetBufferLists;
            FILTER_RELEASE_LOCK(&pFilter->Lock, DispatchLevel);
        }

        NdisFIndicateReceiveNetBufferLists(
                   pFilter->FilterHandle,
                   NetBufferLists,
                   PortNumber,
                   NumberOfNetBufferLists,
                   ReceiveFlags);


        if (NDIS_TEST_RECEIVE_CANNOT_PEND(ReceiveFlags) &&
            pFilter->TrackReceives)
        {
            FILTER_ACQUIRE_LOCK(&pFilter->Lock, DispatchLevel);
            pFilter->OutstandingRcvs -= NumberOfNetBufferLists;
            Ref = pFilter->OutstandingRcvs;
            FILTER_LOG_RCV_REF(2, pFilter, NetBufferLists, Ref);
            FILTER_RELEASE_LOCK(&pFilter->Lock, DispatchLevel);
        }

    } while (bFalse);

    DEBUGP(DL_TRACE, "<===ReceiveNetBufferList: Flags = %8x.\n", ReceiveFlags);

}


_Use_decl_annotations_
VOID
FilterCancelSendNetBufferLists(
    NDIS_HANDLE             FilterModuleContext,
    PVOID                   CancelId
    )
/*++

Routine Description:

    Completes the queued NBLs that carry CancelId (aborted), then passes
    the cancel down.

--*/
{
    PMS_FILTER          pFilter = (PMS_FILTER)FilterModuleContext;
    PNET_BUFFER_LIST    Cancelled = NULL;
    PNET_BUFFER_LIST   *CancelledTail = &Cancelled;
    ULONG               Kept = 0;
    ULONG               i;
    ULONG               Count = 0;
    ULONG64             Bytes = 0;
    BOOLEAN             bFalse = FALSE;

    FILTER_ACQUIRE_LOCK(&pFilter->Lock, bFalse);
    for (i = 0; i < pFilter->PaceRingCount; i++)
    {
        PACER_ENTRY Entry = pFilter->PaceRing[(pFilter->PaceRingHead + i) % PACER_RING_SIZE];
        if (NDIS_GET_NET_BUFFER_LIST_CANCEL_ID(Entry.Nbl) == CancelId)
        {
            Count++;
            Bytes += pacerNblBytes(Entry.Nbl);
            NET_BUFFER_LIST_NEXT_NBL(Entry.Nbl) = NULL;
            *CancelledTail = Entry.Nbl;
            CancelledTail = &NET_BUFFER_LIST_NEXT_NBL(Entry.Nbl);
        }
        else
        {
            pFilter->PaceRing[(pFilter->PaceRingHead + Kept) % PACER_RING_SIZE] = Entry;
            Kept++;
        }
    }
    pFilter->PaceRingCount = Kept;
    pacer_drop_queued(&pFilter->Pacer, Count, Bytes);
    FILTER_RELEASE_LOCK(&pFilter->Lock, bFalse);

    pacerComplete(pFilter, Cancelled, TRUE, NDIS_STATUS_SEND_ABORTED, FALSE, 0);

    NdisFCancelSendNetBufferLists(pFilter->FilterHandle, CancelId);
}

_Use_decl_annotations_
NDIS_STATUS
FilterSetModuleOptions(
    NDIS_HANDLE             FilterModuleContext
    )
/*++

Routine Description:

    This function set the optional handlers for the filter

Arguments:

    FilterModuleContext: The FilterModuleContext given to NdisFSetAttributes

Return Value:

    NDIS_STATUS_SUCCESS
    NDIS_STATUS_RESOURCES
    NDIS_STATUS_FAILURE

--*/
{
   PMS_FILTER                               pFilter = (PMS_FILTER)FilterModuleContext;
   NDIS_FILTER_PARTIAL_CHARACTERISTICS      OptionalHandlers;
   NDIS_STATUS                              Status = NDIS_STATUS_SUCCESS;
   BOOLEAN                                  bFalse = FALSE;

   //
   // Demonstrate how to change send/receive handlers at runtime.
   //
   if (bFalse)
   {
       UINT      i;


       pFilter->CallsRestart++;

       i = pFilter->CallsRestart % 8;

       pFilter->TrackReceives = TRUE;
       pFilter->TrackSends = TRUE;

       NdisMoveMemory(&OptionalHandlers, &DefaultChars, sizeof(OptionalHandlers));
       OptionalHandlers.Header.Type = NDIS_OBJECT_TYPE_FILTER_PARTIAL_CHARACTERISTICS;
       OptionalHandlers.Header.Size = sizeof(OptionalHandlers);
       switch (i)
       {

            case 0:
                OptionalHandlers.ReceiveNetBufferListsHandler = NULL;
                pFilter->TrackReceives = FALSE;
                break;

            case 1:

                OptionalHandlers.ReturnNetBufferListsHandler = NULL;
                pFilter->TrackReceives = FALSE;
                break;

            case 2:
                OptionalHandlers.SendNetBufferListsHandler = NULL;
                pFilter->TrackSends = FALSE;
                break;

            case 3:
                OptionalHandlers.SendNetBufferListsCompleteHandler = NULL;
                pFilter->TrackSends = FALSE;
                break;

            case 4:
                OptionalHandlers.ReceiveNetBufferListsHandler = NULL;
                OptionalHandlers.ReturnNetBufferListsHandler = NULL;
                break;

            case 5:
                OptionalHandlers.SendNetBufferListsHandler = NULL;
                OptionalHandlers.SendNetBufferListsCompleteHandler = NULL;
                break;

            case 6:

                OptionalHandlers.ReceiveNetBufferListsHandler = NULL;
                OptionalHandlers.ReturnNetBufferListsHandler = NULL;
                OptionalHandlers.SendNetBufferListsHandler = NULL;
                OptionalHandlers.SendNetBufferListsCompleteHandler = NULL;
                break;

            case 7:
                break;
       }
       Status = NdisSetOptionalHandlers(pFilter->FilterHandle, (PNDIS_DRIVER_OPTIONAL_HANDLERS)&OptionalHandlers );
   }
   return Status;
}



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
    )
/*++

Routine Description:

    Utility routine that forms and sends an NDIS_OID_REQUEST to the
    miniport, waits for it to complete, and returns status
    to the caller.

    NOTE: this assumes that the calling routine ensures validity
    of the filter handle until this returns.

Arguments:

    FilterModuleContext - pointer to our filter module context
    RequestType - NdisRequest[Set|Query|method]Information
    Oid - the object being set/queried
    InformationBuffer - data for the request
    InformationBufferLength - length of the above
    OutputBufferLength  - valid only for method request
    MethodId - valid only for method request
    pBytesProcessed - place to return bytes read/written

Return Value:

    Status of the set/query request

--*/
{
    FILTER_REQUEST              FilterRequest;
    PNDIS_OID_REQUEST           NdisRequest = &FilterRequest.Request;
    NDIS_STATUS                 Status;
    BOOLEAN                     bFalse;


    bFalse = FALSE;
    *pBytesProcessed = 0;
    NdisZeroMemory(NdisRequest, sizeof(NDIS_OID_REQUEST));

    NdisInitializeEvent(&FilterRequest.ReqEvent);

    NdisRequest->Header.Type = NDIS_OBJECT_TYPE_OID_REQUEST;
    NdisRequest->Header.Revision = NDIS_OID_REQUEST_REVISION_1;
    NdisRequest->Header.Size = sizeof(NDIS_OID_REQUEST);
    NdisRequest->RequestType = RequestType;

    switch (RequestType)
    {
        case NdisRequestQueryInformation:
             NdisRequest->DATA.QUERY_INFORMATION.Oid = Oid;
             NdisRequest->DATA.QUERY_INFORMATION.InformationBuffer =
                                    InformationBuffer;
             NdisRequest->DATA.QUERY_INFORMATION.InformationBufferLength =
                                    InformationBufferLength;
            break;

        case NdisRequestSetInformation:
             NdisRequest->DATA.SET_INFORMATION.Oid = Oid;
             NdisRequest->DATA.SET_INFORMATION.InformationBuffer =
                                    InformationBuffer;
             NdisRequest->DATA.SET_INFORMATION.InformationBufferLength =
                                    InformationBufferLength;
            break;

        case NdisRequestMethod:
             NdisRequest->DATA.METHOD_INFORMATION.Oid = Oid;
             NdisRequest->DATA.METHOD_INFORMATION.MethodId = MethodId;
             NdisRequest->DATA.METHOD_INFORMATION.InformationBuffer =
                                    InformationBuffer;
             NdisRequest->DATA.METHOD_INFORMATION.InputBufferLength =
                                    InformationBufferLength;
             NdisRequest->DATA.METHOD_INFORMATION.OutputBufferLength = OutputBufferLength;
             break;



        default:
            FILTER_ASSERT(bFalse);
            break;
    }

    NdisRequest->RequestId = (PVOID)FILTER_REQUEST_ID;

    Status = NdisFOidRequest(FilterModuleContext->FilterHandle,
                            NdisRequest);


    if (Status == NDIS_STATUS_PENDING)
    {

        NdisWaitEvent(&FilterRequest.ReqEvent, 0);
        Status = FilterRequest.Status;
    }


    if (Status == NDIS_STATUS_SUCCESS)
    {
        if (RequestType == NdisRequestSetInformation)
        {
            *pBytesProcessed = NdisRequest->DATA.SET_INFORMATION.BytesRead;
        }

        if (RequestType == NdisRequestQueryInformation)
        {
            *pBytesProcessed = NdisRequest->DATA.QUERY_INFORMATION.BytesWritten;
        }

        if (RequestType == NdisRequestMethod)
        {
            *pBytesProcessed = NdisRequest->DATA.METHOD_INFORMATION.BytesWritten;
        }

        //
        // The driver below should set the correct value to BytesWritten
        // or BytesRead. But now, we just truncate the value to InformationBufferLength
        //
        if (RequestType == NdisRequestMethod)
        {
            if (*pBytesProcessed > OutputBufferLength)
            {
                *pBytesProcessed = OutputBufferLength;
            }
        }
        else
        {

            if (*pBytesProcessed > InformationBufferLength)
            {
                *pBytesProcessed = InformationBufferLength;
            }
        }
    }


    return Status;
}

VOID
filterInternalRequestComplete(
    _In_ NDIS_HANDLE                  FilterModuleContext,
    _In_ PNDIS_OID_REQUEST            NdisRequest,
    _In_ NDIS_STATUS                  Status
    )
/*++

Routine Description:

    NDIS entry point indicating completion of a pended NDIS_OID_REQUEST.

Arguments:

    FilterModuleContext - pointer to filter module context
    NdisRequest - pointer to NDIS request
    Status - status of request completion

Return Value:

    None

--*/
{
    PFILTER_REQUEST              FilterRequest;


    UNREFERENCED_PARAMETER(FilterModuleContext);

    //
    //  Get at the request context.
    //
    FilterRequest = CONTAINING_RECORD(NdisRequest, FILTER_REQUEST, Request);

    //
    //  Save away the completion status.
    //
    FilterRequest->Status = Status;

    //
    //  Wake up the thread blocked for this request to complete.
    //
    NdisSetEvent(&FilterRequest->ReqEvent);
}



//
// The kernel test stream (ktx, dasio#1): UDP frames the filter builds in
// slots of its own and hands straight to the miniport from a high
// resolution timer, below tcpip and WFP. It shows whether a frame sent
// from the kernel goes out on time while the socket path stalls.
//

// Slots, their MDLs, the NBL pool and the timer. Any failure leaves the
// filter without a test stream (KtxReady FALSE), never without the pacer.
static VOID
ktxSetup(
    _In_ PMS_FILTER Filter,
    _In_ PNDIS_FILTER_ATTACH_PARAMETERS AttachParameters
    )
{
    ULONG i;
    NET_BUFFER_LIST_POOL_PARAMETERS PoolParams;

    Filter->KtxReady = FALSE;
    Filter->KtxActive = FALSE;
    if (AttachParameters->MacAddressLength == 6)
    {
        NdisMoveMemory(Filter->KtxSrcMac, AttachParameters->CurrentMacAddress, 6);
    }
    Filter->KtxBuf = (PUCHAR)NdisAllocateMemoryWithTagPriority(
        FilterDriverHandle, KTX_SLOTS * KTX_SLOT_BYTES, FILTER_ALLOC_TAG, NormalPoolPriority);
    if (Filter->KtxBuf == NULL)
    {
        return;
    }
    for (i = 0; i < KTX_SLOTS; i++)
    {
        Filter->KtxMdl[i] = NdisAllocateMdl(Filter->FilterHandle, Filter->KtxBuf + i * KTX_SLOT_BYTES, KTX_SLOT_BYTES);
        if (Filter->KtxMdl[i] == NULL)
        {
            ktxTeardown(Filter);
            return;
        }
        Filter->KtxFree[i] = i;
    }
    Filter->KtxFreeCount = KTX_SLOTS;

    NdisZeroMemory(&PoolParams, sizeof(PoolParams));
    PoolParams.Header.Type = NDIS_OBJECT_TYPE_DEFAULT;
    PoolParams.Header.Revision = NET_BUFFER_LIST_POOL_PARAMETERS_REVISION_1;
    PoolParams.Header.Size = NDIS_SIZEOF_NET_BUFFER_LIST_POOL_PARAMETERS_REVISION_1;
    PoolParams.ProtocolId = NDIS_PROTOCOL_ID_DEFAULT;
    PoolParams.fAllocateNetBuffer = TRUE;
    PoolParams.ContextSize = PACER_CHILD_CONTEXT;
    PoolParams.PoolTag = FILTER_ALLOC_TAG;
    PoolParams.DataSize = 0;
    Filter->KtxPool = NdisAllocateNetBufferListPool(Filter->FilterHandle, &PoolParams);
    Filter->KtxTimer = ExAllocateTimer(ktxTimer, Filter, EX_TIMER_HIGH_RESOLUTION);
    if (Filter->KtxPool == NULL || Filter->KtxTimer == NULL)
    {
        ktxTeardown(Filter);
        return;
    }
    Filter->KtxReady = TRUE;
}

// Frees what ktxSetup made. At PASSIVE, with no stream and no frame out
// (detach after pause, or a failed setup).
static VOID
ktxTeardown(
    _In_ PMS_FILTER Filter
    )
{
    ULONG i;

    Filter->KtxReady = FALSE;
    Filter->KtxActive = FALSE;
    if (Filter->KtxTimer != NULL)
    {
        ExDeleteTimer(Filter->KtxTimer, TRUE, TRUE, NULL);
        Filter->KtxTimer = NULL;
    }
    if (Filter->KtxPool != NULL)
    {
        NdisFreeNetBufferListPool(Filter->KtxPool);
        Filter->KtxPool = NULL;
    }
    for (i = 0; i < KTX_SLOTS; i++)
    {
        if (Filter->KtxMdl[i] != NULL)
        {
            NdisFreeMdl(Filter->KtxMdl[i]);
            Filter->KtxMdl[i] = NULL;
        }
    }
    Filter->KtxFreeCount = 0;
    if (Filter->KtxBuf != NULL)
    {
        NdisFreeMemory(Filter->KtxBuf, 0, 0);
        Filter->KtxBuf = NULL;
    }
}

_Use_decl_annotations_
NTSTATUS
ktxStartLocked(
    PMS_FILTER Filter,
    const DASIO_KTX_START *Start
    )
{
    ULONG64 Now;

    if (!Filter->KtxReady || Filter->State != FilterRunning)
    {
        return STATUS_DEVICE_NOT_READY;
    }
    if (Start->PayloadBytes < PACER_KTX_MIN_PAYLOAD || Start->PayloadBytes > PACER_KTX_MAX_PAYLOAD ||
        Start->IntervalUs < 50 || Start->IntervalUs > 1000000 ||
        Start->DurationS < 1 || Start->DurationS > 86400)
    {
        return STATUS_INVALID_PARAMETER;
    }
    NdisMoveMemory(Filter->KtxAddr.dst_mac, Start->DstMac, 6);
    NdisMoveMemory(Filter->KtxAddr.src_mac, Filter->KtxSrcMac, 6);
    NdisMoveMemory(Filter->KtxAddr.dst_ip, Start->DstIp, 4);
    NdisMoveMemory(Filter->KtxAddr.src_ip, Start->SrcIp, 4);
    Filter->KtxAddr.dst_port = Start->DstPort;
    Filter->KtxAddr.src_port = Start->SrcPort;
    Filter->KtxAddr.dscp = Start->Dscp;
    Filter->KtxPayload = Start->PayloadBytes;
    Filter->KtxInterval = (ULONG64)Start->IntervalUs * 10;
    Now = pacerNow();
    // The first frame 1 ms from now; the timer's first fire then.
    Filter->KtxStart = Now + KTX_TIMER_PERIOD;
    Filter->KtxEnd = Filter->KtxStart + (ULONG64)Start->DurationS * 10000000;
    // Completed and Failed count every frame of the slots, media TX's
    // included, and stay across test streams: a test stream started in
    // production does not wipe the failure count.
    Filter->KtxSent = 0;
    Filter->KtxNoSlot = 0;
    Filter->KtxFires = 0;
    Filter->KtxMaxBatch = 0;
    Filter->KtxMaxLate = 0;
    Filter->KtxActive = TRUE;
    ExSetTimer(Filter->KtxTimer, -(LONG64)KTX_TIMER_PERIOD, KTX_TIMER_PERIOD, NULL);
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
VOID
ktxStop(
    PMS_FILTER Filter
    )
{
    BOOLEAN bFalse = FALSE;
    FILTER_ACQUIRE_LOCK(&Filter->Lock, bFalse);
    Filter->KtxActive = FALSE;
    FILTER_RELEASE_LOCK(&Filter->Lock, bFalse);
    if (Filter->KtxTimer != NULL)
    {
        ExCancelTimer(Filter->KtxTimer, NULL);
    }
}

// The timer (DISPATCH_LEVEL): builds every frame due by now in free slots
// and sends them down in one chain. A late fire sends the frames it missed
// at once; with every slot out a due frame is dropped and counted (its
// sequence number is skipped, so the receiver sees the hole).
_Use_decl_annotations_
static VOID
ktxTimer(
    PEX_TIMER Timer,
    PVOID Context
    )
{
    PMS_FILTER Filter = (PMS_FILTER)Context;
    PNET_BUFFER_LIST Head = NULL;
    PNET_BUFFER_LIST *Tail = &Head;
    ULONG64 Now = pacerNow();
    BOOLEAN bTrue = TRUE;
    BOOLEAN Stop = FALSE;
    ULONG Due;
    ULONG Batch = 0;
    ULONG i;

    UNREFERENCED_PARAMETER(Timer);
    FILTER_ACQUIRE_LOCK(&Filter->Lock, bTrue);
    if (!Filter->KtxActive || Filter->State != FilterRunning)
    {
        FILTER_RELEASE_LOCK(&Filter->Lock, bTrue);
        return;
    }
    Filter->KtxFires++;
    if (Now >= Filter->KtxEnd)
    {
        Filter->KtxActive = FALSE;
        Stop = TRUE;
    }
    else
    {
        Due = pacer_ktx_due(Filter->KtxStart, Now, Filter->KtxInterval, Filter->KtxSent, KTX_MAX_BATCH);
        if (Due > 0)
        {
            ULONG64 First = Filter->KtxStart + Filter->KtxSent * Filter->KtxInterval;
            if (Now - First > Filter->KtxMaxLate)
            {
                Filter->KtxMaxLate = Now - First;
            }
        }
        for (i = 0; i < Due; i++)
        {
            ULONG Slot;
            ULONG Len;
            PNET_BUFFER_LIST Nbl;
            if (Filter->KtxFreeCount == 0)
            {
                Filter->KtxNoSlot++;
                Filter->KtxSent++;
                continue;
            }
            Slot = Filter->KtxFree[--Filter->KtxFreeCount];
            Len = pacer_ktx_frame(Filter->KtxBuf + Slot * KTX_SLOT_BYTES, &Filter->KtxAddr, Filter->KtxPayload,
                                  (USHORT)Filter->KtxSent, Filter->KtxSent, Now);
            Nbl = NdisAllocateNetBufferAndNetBufferList(Filter->KtxPool, PACER_CHILD_CONTEXT, 0,
                                                        Filter->KtxMdl[Slot], 0, Len);
            if (Nbl == NULL)
            {
                Filter->KtxFree[Filter->KtxFreeCount++] = Slot;
                Filter->KtxNoSlot++;
                Filter->KtxSent++;
                continue;
            }
            Nbl->SourceHandle = Filter->FilterHandle;
            *(PULONG)NET_BUFFER_LIST_CONTEXT_DATA_START(Nbl) = Slot;
            *Tail = Nbl;
            Tail = &NET_BUFFER_LIST_NEXT_NBL(Nbl);
            Filter->KtxSent++;
            Filter->KtxOut++;
            Batch++;
        }
        if (Batch > Filter->KtxMaxBatch)
        {
            Filter->KtxMaxBatch = Batch;
        }
    }
    FILTER_RELEASE_LOCK(&Filter->Lock, bTrue);

    if (Stop)
    {
        ExCancelTimer(Filter->KtxTimer, NULL);
    }
    if (Head != NULL)
    {
        NdisFSendNetBufferLists(Filter->FilterHandle, Head, NDIS_DEFAULT_PORT_NUMBER, NDIS_SEND_FLAGS_DISPATCH_LEVEL);
    }
}

ULONG TxMaxOutSetting = 0;

// Media TX (dasio#1): the dasio service's own audio frames, copied into
// free test stream slots and sent down in one chain. Their completions go
// through ktxDone like the test stream's (same pool), and pause waits for
// them (KtxOut).
_Use_decl_annotations_
NTSTATUS
txSendAtDispatch(
    PMS_FILTER Filter,
    const UCHAR *Input,
    ULONG Length,
    PULONG Accepted
    )
{
    PNET_BUFFER_LIST Head = NULL;
    PNET_BUFFER_LIST *Tail = &Head;
    BOOLEAN bTrue = TRUE;
    uint32_t At = 0;
    uint32_t Off;
    uint32_t Len;
    ULONG Total = 0;
    ULONG Batch = 0;
    int r;

    *Accepted = 0;
    // The whole input first: a malformed one sends nothing.
    while ((r = pacer_tx_next(Input, Length, &At, &Off, &Len)) == 1)
    {
        Total++;
    }
    if (r < 0)
    {
        return STATUS_INVALID_PARAMETER;
    }
    FILTER_ACQUIRE_LOCK(&Filter->Lock, bTrue);
    if (!Filter->KtxReady || Filter->State != FilterRunning)
    {
        FILTER_RELEASE_LOCK(&Filter->Lock, bTrue);
        return STATUS_DEVICE_NOT_READY;
    }
    At = 0;
    while (pacer_tx_next(Input, Length, &At, &Off, &Len) == 1)
    {
        ULONG Slot;
        PUCHAR Frame;
        PNET_BUFFER_LIST Nbl;
        if (Filter->KtxFreeCount == 0 || (TxMaxOutSetting != 0 && Filter->KtxOut >= TxMaxOutSetting))
        {
            break;
        }
        Slot = Filter->KtxFree[--Filter->KtxFreeCount];
        Frame = Filter->KtxBuf + Slot * KTX_SLOT_BYTES;
        NdisMoveMemory(Frame, Input + Off, Len);
        NdisMoveMemory(Frame + 6, Filter->KtxSrcMac, 6);
        Nbl = NdisAllocateNetBufferAndNetBufferList(Filter->KtxPool, PACER_CHILD_CONTEXT, 0,
                                                    Filter->KtxMdl[Slot], 0, Len);
        if (Nbl == NULL)
        {
            Filter->KtxFree[Filter->KtxFreeCount++] = Slot;
            break;
        }
        Nbl->SourceHandle = Filter->FilterHandle;
        *(PULONG)NET_BUFFER_LIST_CONTEXT_DATA_START(Nbl) = Slot;
        *Tail = Nbl;
        Tail = &NET_BUFFER_LIST_NEXT_NBL(Nbl);
        // Media frames count in TxFrames only: KtxSent is the test stream's
        // schedule (pacer_ktx_due).
        Filter->KtxOut++;
        Batch++;
    }
    if (Filter->KtxOut > Filter->KtxMaxOut)
    {
        Filter->KtxMaxOut = Filter->KtxOut;
    }
    Filter->TxFrames += Batch;
    Filter->TxNoSlot += Total - Batch;
    if (Total > Batch)
    {
        ULONG64 Now = KeQueryInterruptTime();
        if (Filter->TxLastNoSlotTime == 0 || Now - Filter->TxLastNoSlotTime > 10000000)
        {
            Filter->TxNoSlotEvents++;
        }
        Filter->TxLastNoSlotTime = Now;
    }
    if (Batch > 0)
    {
        Filter->TxBatches++;
    }
    if (Batch > Filter->TxMaxBatch)
    {
        Filter->TxMaxBatch = Batch;
    }
    FILTER_RELEASE_LOCK(&Filter->Lock, bTrue);

    if (Head != NULL)
    {
        NdisFSendNetBufferLists(Filter->FilterHandle, Head, NDIS_DEFAULT_PORT_NUMBER, NDIS_SEND_FLAGS_DISPATCH_LEVEL);
    }
    *Accepted = Batch;
    return STATUS_SUCCESS;
}

// A test stream frame completed: its NBL is freed and its slot is free.
static VOID
ktxDone(
    _In_ PMS_FILTER Filter,
    _In_ PNET_BUFFER_LIST Nbl,
    _In_ BOOLEAN DispatchLevel
    )
{
    ULONG Slot = *(PULONG)NET_BUFFER_LIST_CONTEXT_DATA_START(Nbl);
    NDIS_STATUS Status = NET_BUFFER_LIST_STATUS(Nbl);

    NdisFreeNetBufferList(Nbl);
    FILTER_ACQUIRE_LOCK(&Filter->Lock, DispatchLevel);
    if (Slot < KTX_SLOTS && Filter->KtxFreeCount < KTX_SLOTS)
    {
        Filter->KtxFree[Filter->KtxFreeCount++] = Slot;
    }
    Filter->KtxCompleted++;
    if (Status != NDIS_STATUS_SUCCESS)
    {
        ULONG64 Now = KeQueryInterruptTime();
        Filter->KtxFailed++;
        if (Filter->KtxLastFailTime == 0 || Now - Filter->KtxLastFailTime > 10000000)
        {
            Filter->KtxFailEvents++;
        }
        Filter->KtxLastFailStatus = (ULONG)Status;
        Filter->KtxLastFailTime = Now;
    }
    if (Filter->KtxOut > 0)
    {
        Filter->KtxOut--;
    }
    FILTER_RELEASE_LOCK(&Filter->Lock, DispatchLevel);
}


//
// Deferred receive (rxd, dasio#1): the NIC's receive DPC queues its NBLs
// here and returns; this DPC, on its own CPU, indicates them up in order.
//

ULONG RxdEnable = 0;
LONG RxdCpuSetting = -1;

// The settings onto one filter (outside its Lock: the DPC's target is set
// with no lock held, before the flag that lets receives queue).
_Use_decl_annotations_
VOID
rxdApply(
    PMS_FILTER Filter
    )
{
    BOOLEAN bFalse = FALSE;
    LONG Cpu = RxdCpuSetting;
    if (Cpu >= 0 && (ULONG)Cpu < KeQueryActiveProcessorCountEx(ALL_PROCESSOR_GROUPS))
    {
        PROCESSOR_NUMBER Number;
        if (NT_SUCCESS(KeGetProcessorNumberFromIndex((ULONG)Cpu, &Number)) &&
            NT_SUCCESS(KeSetTargetProcessorDpcEx(&Filter->RxdDpc, &Number)))
        {
            Filter->RxdCpu = Cpu;
        }
    }
    FILTER_ACQUIRE_LOCK(&Filter->Lock, bFalse);
    Filter->RxdEnabled = RxdEnable != 0;
    FILTER_RELEASE_LOCK(&Filter->Lock, bFalse);
}

// Indicates everything queued, in order, as one chain per run.
_Use_decl_annotations_
static VOID
rxdDpc(
    PKDPC Dpc,
    PVOID DeferredContext,
    PVOID SystemArgument1,
    PVOID SystemArgument2
    )
{
    PMS_FILTER Filter = (PMS_FILTER)DeferredContext;
    PNET_BUFFER_LIST Chain;
    ULONG Count;
    BOOLEAN bTrue = TRUE;

    UNREFERENCED_PARAMETER(Dpc);
    UNREFERENCED_PARAMETER(SystemArgument1);
    UNREFERENCED_PARAMETER(SystemArgument2);
    for (;;)
    {
        FILTER_ACQUIRE_LOCK(&Filter->Lock, bTrue);
        Chain = Filter->RxdHead;
        Count = Filter->RxdQueued;
        Filter->RxdHead = NULL;
        Filter->RxdTail = &Filter->RxdHead;
        Filter->RxdQueued = 0;
        if (Chain != NULL)
        {
            Filter->RxdDpcRuns++;
            if (Count > Filter->RxdMaxBatch)
            {
                Filter->RxdMaxBatch = Count;
            }
        }
        FILTER_RELEASE_LOCK(&Filter->Lock, bTrue);
        if (Chain == NULL)
        {
            return;
        }
        NdisFIndicateReceiveNetBufferLists(Filter->FilterHandle, Chain, NDIS_DEFAULT_PORT_NUMBER, Count,
                                           NDIS_RECEIVE_FLAGS_DISPATCH_LEVEL);
    }
}

// Pause: what waits goes back to the miniport unindicated.
static VOID
rxdDrop(
    _In_ PMS_FILTER Filter
    )
{
    PNET_BUFFER_LIST Chain;
    ULONG Count;
    BOOLEAN bFalse = FALSE;

    FILTER_ACQUIRE_LOCK(&Filter->Lock, bFalse);
    Chain = Filter->RxdHead;
    Count = Filter->RxdQueued;
    Filter->RxdHead = NULL;
    Filter->RxdTail = &Filter->RxdHead;
    Filter->RxdQueued = 0;
    Filter->RxdDropped += Count;
    if (Filter->TrackReceives)
    {
        Filter->OutstandingRcvs -= Count;
    }
    FILTER_RELEASE_LOCK(&Filter->Lock, bFalse);
    if (Chain != NULL)
    {
        NdisFReturnNetBufferLists(Filter->FilterHandle, Chain, 0);
    }
}


// RxdEnable and RxdCpu (REG_DWORD; RxdCpu 0xFFFFFFFF = -1) from
// <service>\Parameters. Missing key or values: off, as before.
static VOID
rxdReadRegistry(
    _In_ PUNICODE_STRING RegistryPath
    )
{
    RTL_QUERY_REGISTRY_TABLE Table[5];
    ULONG Enable = 0;
    ULONG MaxOut = 0;
    ULONG DefaultMaxOut = 0;
    ULONG Cpu = (ULONG)-1;
    ULONG DefaultEnable = 0;
    ULONG DefaultCpu = (ULONG)-1;
    PWCHAR Path;
    USHORT Chars = RegistryPath->Length / sizeof(WCHAR);

    // The path NUL-terminated (a UNICODE_STRING need not be).
    Path = (PWCHAR)ExAllocatePool2(POOL_FLAG_PAGED, (Chars + 1) * sizeof(WCHAR), FILTER_ALLOC_TAG);
    if (Path == NULL)
    {
        return;
    }
    RtlCopyMemory(Path, RegistryPath->Buffer, Chars * sizeof(WCHAR));
    Path[Chars] = L'\0';

    RtlZeroMemory(Table, sizeof(Table));
    Table[0].Flags = RTL_QUERY_REGISTRY_SUBKEY;
    Table[0].Name = L"Parameters";
    Table[1].Flags = RTL_QUERY_REGISTRY_DIRECT | RTL_QUERY_REGISTRY_TYPECHECK;
    Table[1].Name = L"RxdEnable";
    Table[1].EntryContext = &Enable;
    Table[1].DefaultType = (REG_DWORD << RTL_QUERY_REGISTRY_TYPECHECK_SHIFT) | REG_DWORD;
    Table[1].DefaultData = &DefaultEnable;
    Table[1].DefaultLength = sizeof(ULONG);
    Table[2].Flags = RTL_QUERY_REGISTRY_DIRECT | RTL_QUERY_REGISTRY_TYPECHECK;
    Table[2].Name = L"RxdCpu";
    Table[2].EntryContext = &Cpu;
    Table[2].DefaultType = (REG_DWORD << RTL_QUERY_REGISTRY_TYPECHECK_SHIFT) | REG_DWORD;
    Table[2].DefaultData = &DefaultCpu;
    Table[2].DefaultLength = sizeof(ULONG);
    // Media TX's bound on frames out (0: every slot), kept across reboots.
    Table[3].Flags = RTL_QUERY_REGISTRY_DIRECT | RTL_QUERY_REGISTRY_TYPECHECK;
    Table[3].Name = L"TxMaxOut";
    Table[3].EntryContext = &MaxOut;
    Table[3].DefaultType = (REG_DWORD << RTL_QUERY_REGISTRY_TYPECHECK_SHIFT) | REG_DWORD;
    Table[3].DefaultData = &DefaultMaxOut;
    Table[3].DefaultLength = sizeof(ULONG);
    if (NT_SUCCESS(RtlQueryRegistryValues(RTL_REGISTRY_ABSOLUTE, Path, Table, NULL, NULL)))
    {
        RxdEnable = Enable != 0;
        RxdCpuSetting = (LONG)Cpu;
        TxMaxOutSetting = MaxOut <= KTX_SLOTS ? MaxOut : 0;
    }
    ExFreePoolWithTag(Path, FILTER_ALLOC_TAG);
}
