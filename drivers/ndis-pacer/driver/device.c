/*++
 *
 * The file contains the routines to create a device and handle ioctls
 *
-- */

#include "precomp.h"

// The control device's class (IoCreateDeviceSecure, through NDIS, keys an
// administrator's ACL override by it).
static const GUID DasioPacerDeviceClass =
    { 0x61b677a7, 0xf3f5, 0x438c, { 0x9a, 0x46, 0x5d, 0x2f, 0x74, 0xc5, 0x6b, 0xad } };


#pragma NDIS_INIT_FUNCTION(FilterRegisterDevice)


_IRQL_requires_max_(PASSIVE_LEVEL)
NDIS_STATUS
FilterRegisterDevice(
    VOID
    )
{
    NDIS_STATUS            Status = NDIS_STATUS_SUCCESS;
    UNICODE_STRING         DeviceName;
    UNICODE_STRING         DeviceLinkUnicodeString;
    UNICODE_STRING         DeviceSddl;
    PDRIVER_DISPATCH       DispatchTable[IRP_MJ_MAXIMUM_FUNCTION+1];
    NDIS_DEVICE_OBJECT_ATTRIBUTES   DeviceAttribute;
    PFILTER_DEVICE_EXTENSION        FilterDeviceExtension;

    DEBUGP(DL_TRACE, "==>FilterRegisterDevice\n");

    NdisZeroMemory(DispatchTable, (IRP_MJ_MAXIMUM_FUNCTION+1) * sizeof(PDRIVER_DISPATCH));

    DispatchTable[IRP_MJ_CREATE] = FilterDispatch;
    DispatchTable[IRP_MJ_CLEANUP] = FilterDispatch;
    DispatchTable[IRP_MJ_CLOSE] = FilterDispatch;
    DispatchTable[IRP_MJ_DEVICE_CONTROL] = FilterDeviceIoControl;


    NdisInitUnicodeString(&DeviceName, NTDEVICE_STRING);
    NdisInitUnicodeString(&DeviceLinkUnicodeString, LINKNAME_STRING);
    // SYSTEM and Administrators only (dasio#1): IOCTL_DASIO_TX_SEND puts
    // frames on the wire, so no other user may open the device.
    NdisInitUnicodeString(&DeviceSddl, L"D:P(A;;GA;;;SY)(A;;GA;;;BA)");

    //
    // Create a device object and register our dispatch handlers
    //
    NdisZeroMemory(&DeviceAttribute, sizeof(NDIS_DEVICE_OBJECT_ATTRIBUTES));

    DeviceAttribute.Header.Type = NDIS_OBJECT_TYPE_DEVICE_OBJECT_ATTRIBUTES;
    DeviceAttribute.Header.Revision = NDIS_DEVICE_OBJECT_ATTRIBUTES_REVISION_1;
    DeviceAttribute.Header.Size = sizeof(NDIS_DEVICE_OBJECT_ATTRIBUTES);

    DeviceAttribute.DeviceName = &DeviceName;
    DeviceAttribute.SymbolicName = &DeviceLinkUnicodeString;
    DeviceAttribute.MajorFunctions = &DispatchTable[0];
    DeviceAttribute.ExtensionSize = sizeof(FILTER_DEVICE_EXTENSION);
    DeviceAttribute.DefaultSDDLString = &DeviceSddl;
    DeviceAttribute.DeviceClassGuid = &DasioPacerDeviceClass;

    Status = NdisRegisterDeviceEx(
                FilterDriverHandle,
                &DeviceAttribute,
                &NdisDeviceObject,
                &NdisFilterDeviceHandle
                );


    if (Status == NDIS_STATUS_SUCCESS)
    {
        FilterDeviceExtension = NdisGetDeviceReservedExtension(NdisDeviceObject);

        FilterDeviceExtension->Signature = 'FTDR';
        FilterDeviceExtension->Handle = FilterDriverHandle;
    }


    DEBUGP(DL_TRACE, "<==FilterRegisterDevice: %x\n", Status);

    return (Status);

}

_IRQL_requires_max_(PASSIVE_LEVEL)
VOID
FilterDeregisterDevice(
    VOID
    )

{
    if (NdisFilterDeviceHandle != NULL)
    {
        NdisDeregisterDeviceEx(NdisFilterDeviceHandle);
    }

    NdisFilterDeviceHandle = NULL;

}

_Use_decl_annotations_
NTSTATUS
FilterDispatch(
    PDEVICE_OBJECT       DeviceObject,
    PIRP                 Irp
    )
{
    PIO_STACK_LOCATION       IrpStack;
    NTSTATUS                 Status = STATUS_SUCCESS;

    UNREFERENCED_PARAMETER(DeviceObject);

    IrpStack = IoGetCurrentIrpStackLocation(Irp);

    switch (IrpStack->MajorFunction)
    {
        case IRP_MJ_CREATE:
            break;

        case IRP_MJ_CLEANUP:
            break;

        case IRP_MJ_CLOSE:
            break;

        default:
            break;
    }

    Irp->IoStatus.Status = Status;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);

    return Status;
}

_Use_decl_annotations_
NTSTATUS
FilterDeviceIoControl(
    PDEVICE_OBJECT        DeviceObject,
    PIRP                  Irp
    )
{
    PIO_STACK_LOCATION          IrpSp;
    NTSTATUS                    Status = STATUS_SUCCESS;
    PFILTER_DEVICE_EXTENSION    FilterDeviceExtension;
    PUCHAR                      InputBuffer;
    PUCHAR                      OutputBuffer;
    ULONG                       InputBufferLength, OutputBufferLength;
    PLIST_ENTRY                 Link;
    PUCHAR                      pInfo;
    ULONG                       InfoLength = 0;
    PMS_FILTER                  pFilter = NULL;
    BOOLEAN                     bFalse = FALSE;


    UNREFERENCED_PARAMETER(DeviceObject);


    IrpSp = IoGetCurrentIrpStackLocation(Irp);

    if (IrpSp->FileObject == NULL)
    {
        return(STATUS_UNSUCCESSFUL);
    }


    FilterDeviceExtension = (PFILTER_DEVICE_EXTENSION)NdisGetDeviceReservedExtension(DeviceObject);

    ASSERT(FilterDeviceExtension->Signature == 'FTDR');

    Irp->IoStatus.Information = 0;

    switch (IrpSp->Parameters.DeviceIoControl.IoControlCode)
    {

        case IOCTL_FILTER_RESTART_ALL:
            break;

        case IOCTL_FILTER_ENUMERATE_ALL_INSTANCES:

            InputBuffer = OutputBuffer = (PUCHAR)Irp->AssociatedIrp.SystemBuffer;
            InputBufferLength = IrpSp->Parameters.DeviceIoControl.InputBufferLength;
            OutputBufferLength = IrpSp->Parameters.DeviceIoControl.OutputBufferLength;


            pInfo = OutputBuffer;

            FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);

            Link = FilterModuleList.Flink;

            while (Link != &FilterModuleList)
            {
                pFilter = CONTAINING_RECORD(Link, MS_FILTER, FilterModuleLink);


                InfoLength += (pFilter->FilterModuleName.Length + sizeof(USHORT));

                if (InfoLength <= OutputBufferLength)
                {
                    *(PUSHORT)pInfo = pFilter->FilterModuleName.Length;
                    NdisMoveMemory(pInfo + sizeof(USHORT),
                                   (PUCHAR)(pFilter->FilterModuleName.Buffer),
                                   pFilter->FilterModuleName.Length);

                    pInfo += (pFilter->FilterModuleName.Length + sizeof(USHORT));
                }

                Link = Link->Flink;
            }

            FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
            if (InfoLength <= OutputBufferLength)
            {

                Status = NDIS_STATUS_SUCCESS;
            }
            //
            // Buffer is small
            //
            else
            {
                Status = STATUS_BUFFER_TOO_SMALL;
            }
            break;


        case IOCTL_DASIO_PACER_SET:
            // New settings for all bound adapters, applied under each
            // filter's lock (the list lock held outside it).
            InputBuffer = (PUCHAR)Irp->AssociatedIrp.SystemBuffer;
            InputBufferLength = IrpSp->Parameters.DeviceIoControl.InputBufferLength;
            if (InputBufferLength < sizeof(DASIO_PACER_SET) ||
                ((PDASIO_PACER_SET)InputBuffer)->RatePercent > 100)
            {
                Status = STATUS_INVALID_PARAMETER;
                break;
            }
            FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
            PacerRatePercent = ((PDASIO_PACER_SET)InputBuffer)->RatePercent;
            PacerBurstBytes = ((PDASIO_PACER_SET)InputBuffer)->BurstBytes;
            for (Link = FilterModuleList.Flink; Link != &FilterModuleList; Link = Link->Flink)
            {
                BOOLEAN bTrue = TRUE;
                pFilter = CONTAINING_RECORD(Link, MS_FILTER, FilterModuleLink);
                FILTER_ACQUIRE_LOCK(&pFilter->Lock, bTrue);
                pacerApplyRateLocked(pFilter);
                FILTER_RELEASE_LOCK(&pFilter->Lock, bTrue);
            }
            FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
            break;

        case IOCTL_DASIO_PACER_STATUS:
            // One status per bound adapter, each read under its filter's
            // lock (the list lock held outside it, as attach and detach do).
            OutputBuffer = (PUCHAR)Irp->AssociatedIrp.SystemBuffer;
            OutputBufferLength = IrpSp->Parameters.DeviceIoControl.OutputBufferLength;
            FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
            for (Link = FilterModuleList.Flink; Link != &FilterModuleList; Link = Link->Flink)
            {
                PDASIO_PACER_STATUS Out;
                BOOLEAN bTrue = TRUE;
                pFilter = CONTAINING_RECORD(Link, MS_FILTER, FilterModuleLink);
                if (InfoLength + (ULONG)sizeof(DASIO_PACER_STATUS) > OutputBufferLength)
                {
                    Status = STATUS_BUFFER_TOO_SMALL;
                    break;
                }
                Out = (PDASIO_PACER_STATUS)(OutputBuffer + InfoLength);
                FILTER_ACQUIRE_LOCK(&pFilter->Lock, bTrue);
                Out->Version = DASIO_PACER_STATUS_VERSION;
                Out->IfIndex = pFilter->MiniportIfIndex;
                Out->State = (ULONG64)pFilter->State;
                Out->LinkAtAttach = pFilter->PaceLinkAtAttach;
                Out->LinkLast = pFilter->PaceLinkLast;
                Out->LinkUpdates = pFilter->PaceLinkUpdates;
                Out->Rate = pFilter->Pacer.rate;
                Out->Burst = pFilter->Pacer.burst;
                Out->Tokens = pFilter->Pacer.tokens;
                Out->Last = pFilter->Pacer.last;
                Out->Now = KeQueryInterruptTime();
                Out->Queued = pFilter->Pacer.queued;
                Out->QueuedBytes = pFilter->Pacer.queued_bytes;
                Out->RingCount = pFilter->PaceRingCount;
                Out->MaxQueued = pFilter->Pacer.max_queued;
                Out->PrioritySent = pFilter->Pacer.priority_sent;
                Out->Admitted = pFilter->Pacer.admitted;
                Out->Deferred = pFilter->Pacer.deferred;
                Out->TimerArms = pFilter->PaceTimerArms;
                Out->TimerFires = pFilter->PaceTimerFires;
                Out->TimerArmed = pFilter->PaceTimerArmed;
                Out->Pumps = pFilter->PacePumps;
                Out->PumpSent = pFilter->PacePumpSent;
                Out->Pumping = pFilter->PacePumping;
                Out->OtherNbls = pFilter->PaceOtherNbls;
                Out->OtherNbs = pFilter->PaceOtherNbs;
                Out->MaxNbs = pFilter->PaceMaxNbs;
                Out->MaxNblBytes = pFilter->PaceMaxNblBytes;
                Out->Splits = pFilter->PaceSplits;
                Out->SplitChildren = pFilter->PaceSplitChildren;
                Out->SplitFails = pFilter->PaceSplitFails;
                Out->ChildrenOut = pFilter->PaceChildrenOut;
                FILTER_RELEASE_LOCK(&pFilter->Lock, bTrue);
                InfoLength += (ULONG)sizeof(DASIO_PACER_STATUS);
            }
            FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
            if (Status == STATUS_BUFFER_TOO_SMALL)
            {
                InfoLength = 0;
            }
            break;

        case IOCTL_DASIO_KTX_START:
            // A test stream on the adapter with the given IfIndex, started
            // under its filter's lock (the list lock held outside it).
            InputBuffer = (PUCHAR)Irp->AssociatedIrp.SystemBuffer;
            InputBufferLength = IrpSp->Parameters.DeviceIoControl.InputBufferLength;
            if (InputBufferLength < sizeof(DASIO_KTX_START))
            {
                Status = STATUS_INVALID_PARAMETER;
                break;
            }
            Status = STATUS_NOT_FOUND;
            FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
            for (Link = FilterModuleList.Flink; Link != &FilterModuleList; Link = Link->Flink)
            {
                BOOLEAN bTrue = TRUE;
                pFilter = CONTAINING_RECORD(Link, MS_FILTER, FilterModuleLink);
                if (pFilter->MiniportIfIndex != ((PDASIO_KTX_START)InputBuffer)->IfIndex)
                {
                    continue;
                }
                FILTER_ACQUIRE_LOCK(&pFilter->Lock, bTrue);
                Status = ktxStartLocked(pFilter, (PDASIO_KTX_START)InputBuffer);
                FILTER_RELEASE_LOCK(&pFilter->Lock, bTrue);
                break;
            }
            FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
            break;

        case IOCTL_DASIO_KTX_STOP:
            FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
            for (Link = FilterModuleList.Flink; Link != &FilterModuleList; Link = Link->Flink)
            {
                pFilter = CONTAINING_RECORD(Link, MS_FILTER, FilterModuleLink);
                ktxStop(pFilter);
            }
            FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
            break;

        case IOCTL_DASIO_KTX_STATUS:
            // One status per bound adapter, each read under its filter's lock.
            OutputBuffer = (PUCHAR)Irp->AssociatedIrp.SystemBuffer;
            OutputBufferLength = IrpSp->Parameters.DeviceIoControl.OutputBufferLength;
            FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
            for (Link = FilterModuleList.Flink; Link != &FilterModuleList; Link = Link->Flink)
            {
                PDASIO_KTX_STATUS Out;
                BOOLEAN bTrue = TRUE;
                pFilter = CONTAINING_RECORD(Link, MS_FILTER, FilterModuleLink);
                if (InfoLength + (ULONG)sizeof(DASIO_KTX_STATUS) > OutputBufferLength)
                {
                    Status = STATUS_BUFFER_TOO_SMALL;
                    break;
                }
                Out = (PDASIO_KTX_STATUS)(OutputBuffer + InfoLength);
                FILTER_ACQUIRE_LOCK(&pFilter->Lock, bTrue);
                Out->Version = DASIO_KTX_STATUS_VERSION;
                Out->IfIndex = pFilter->MiniportIfIndex;
                Out->Active = pFilter->KtxActive;
                Out->Ready = pFilter->KtxReady;
                Out->Start = pFilter->KtxStart;
                Out->End = pFilter->KtxEnd;
                Out->Now = KeQueryInterruptTime();
                Out->IntervalUs = pFilter->KtxInterval / 10;
                Out->Sent = pFilter->KtxSent;
                Out->Completed = pFilter->KtxCompleted;
                Out->Failed = pFilter->KtxFailed;
                Out->Out = pFilter->KtxOut;
                Out->NoSlot = pFilter->KtxNoSlot;
                Out->Fires = pFilter->KtxFires;
                Out->MaxBatch = pFilter->KtxMaxBatch;
                Out->MaxLate = pFilter->KtxMaxLate;
                Out->TxBatches = pFilter->TxBatches;
                Out->TxFrames = pFilter->TxFrames;
                Out->TxNoSlot = pFilter->TxNoSlot;
                Out->TxMaxBatch = pFilter->TxMaxBatch;
                Out->TxMaxOut = TxMaxOutSetting;
                Out->MaxOut = pFilter->KtxMaxOut;
                Out->FailEvents = pFilter->KtxFailEvents;
                Out->LastFailStatus = pFilter->KtxLastFailStatus;
                Out->LastFailTime = pFilter->KtxLastFailTime;
                FILTER_RELEASE_LOCK(&pFilter->Lock, bTrue);
                InfoLength += (ULONG)sizeof(DASIO_KTX_STATUS);
            }
            FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
            if (Status == STATUS_BUFFER_TOO_SMALL)
            {
                InfoLength = 0;
            }
            break;

        case IOCTL_DASIO_TX_SEND:
            // The dasio service's frames on the adapter with the given
            // IfIndex. Sent with the list lock held: the filter stays
            // attached until its send call returns.
            InputBuffer = OutputBuffer = (PUCHAR)Irp->AssociatedIrp.SystemBuffer;
            InputBufferLength = IrpSp->Parameters.DeviceIoControl.InputBufferLength;
            OutputBufferLength = IrpSp->Parameters.DeviceIoControl.OutputBufferLength;
            if (InputBufferLength < sizeof(DASIO_TX_SEND) || OutputBufferLength < sizeof(ULONG))
            {
                Status = STATUS_INVALID_PARAMETER;
                break;
            }
            {
                ULONG IfIndex = ((PDASIO_TX_SEND)InputBuffer)->IfIndex;
                ULONG Accepted = 0;
                Status = STATUS_NOT_FOUND;
                FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
                for (Link = FilterModuleList.Flink; Link != &FilterModuleList; Link = Link->Flink)
                {
                    pFilter = CONTAINING_RECORD(Link, MS_FILTER, FilterModuleLink);
                    if (pFilter->MiniportIfIndex != IfIndex)
                    {
                        continue;
                    }
                    Status = txSendAtDispatch(pFilter, InputBuffer + sizeof(DASIO_TX_SEND),
                                              InputBufferLength - (ULONG)sizeof(DASIO_TX_SEND), &Accepted);
                    break;
                }
                FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
                if (Status == STATUS_SUCCESS)
                {
                    // The input is consumed: the output overwrites its start.
                    *(PULONG)OutputBuffer = Accepted;
                    InfoLength = sizeof(ULONG);
                }
            }
            break;

        case IOCTL_DASIO_TX_SET:
            InputBuffer = (PUCHAR)Irp->AssociatedIrp.SystemBuffer;
            InputBufferLength = IrpSp->Parameters.DeviceIoControl.InputBufferLength;
            if (InputBufferLength < sizeof(ULONG) || *(PULONG)InputBuffer > KTX_SLOTS)
            {
                Status = STATUS_INVALID_PARAMETER;
                break;
            }
            TxMaxOutSetting = *(PULONG)InputBuffer;
            break;

        case IOCTL_DASIO_RXD_SET:
            // Deferred receive on or off for all bound adapters.
            InputBuffer = (PUCHAR)Irp->AssociatedIrp.SystemBuffer;
            InputBufferLength = IrpSp->Parameters.DeviceIoControl.InputBufferLength;
            if (InputBufferLength < sizeof(DASIO_RXD_SET))
            {
                Status = STATUS_INVALID_PARAMETER;
                break;
            }
            FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
            RxdEnable = ((PDASIO_RXD_SET)InputBuffer)->Enable;
            RxdCpuSetting = ((PDASIO_RXD_SET)InputBuffer)->Cpu;
            for (Link = FilterModuleList.Flink; Link != &FilterModuleList; Link = Link->Flink)
            {
                pFilter = CONTAINING_RECORD(Link, MS_FILTER, FilterModuleLink);
                rxdApply(pFilter);
            }
            FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
            break;

        case IOCTL_DASIO_RXD_STATUS:
            OutputBuffer = (PUCHAR)Irp->AssociatedIrp.SystemBuffer;
            OutputBufferLength = IrpSp->Parameters.DeviceIoControl.OutputBufferLength;
            FILTER_ACQUIRE_LOCK(&FilterListLock, bFalse);
            for (Link = FilterModuleList.Flink; Link != &FilterModuleList; Link = Link->Flink)
            {
                PDASIO_RXD_STATUS Out;
                BOOLEAN bTrue = TRUE;
                pFilter = CONTAINING_RECORD(Link, MS_FILTER, FilterModuleLink);
                if (InfoLength + (ULONG)sizeof(DASIO_RXD_STATUS) > OutputBufferLength)
                {
                    Status = STATUS_BUFFER_TOO_SMALL;
                    break;
                }
                Out = (PDASIO_RXD_STATUS)(OutputBuffer + InfoLength);
                FILTER_ACQUIRE_LOCK(&pFilter->Lock, bTrue);
                Out->Version = DASIO_RXD_STATUS_VERSION;
                Out->IfIndex = pFilter->MiniportIfIndex;
                Out->Enabled = pFilter->RxdEnabled;
                Out->Cpu = pFilter->RxdCpu;
                Out->Deferred = pFilter->RxdDeferred;
                Out->Inline = pFilter->RxdInline;
                Out->DpcRuns = pFilter->RxdDpcRuns;
                Out->MaxBatch = pFilter->RxdMaxBatch;
                Out->Queued = pFilter->RxdQueued;
                Out->Dropped = pFilter->RxdDropped;
                Out->Priority = pFilter->RxdPriority;
                FILTER_RELEASE_LOCK(&pFilter->Lock, bTrue);
                InfoLength += (ULONG)sizeof(DASIO_RXD_STATUS);
            }
            FILTER_RELEASE_LOCK(&FilterListLock, bFalse);
            if (Status == STATUS_BUFFER_TOO_SMALL)
            {
                InfoLength = 0;
            }
            break;

        default:
            break;
    }

    Irp->IoStatus.Status = Status;
    Irp->IoStatus.Information = InfoLength;

    IoCompleteRequest(Irp, IO_NO_INCREMENT);

    return Status;


}






