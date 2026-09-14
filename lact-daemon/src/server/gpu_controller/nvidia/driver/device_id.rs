use std::{mem, os::fd::RawFd};

use super::rm_control;
use crate::bindings::nvidia::{
    NV0000_CTRL_CMD_GPU_GET_ATTACHED_IDS, NV0000_CTRL_CMD_GPU_GET_ID_INFO_V2,
    NV0000_CTRL_CMD_GPU_GET_PCI_INFO, NV0000_CTRL_GPU_GET_ATTACHED_IDS_PARAMS,
    NV0000_CTRL_GPU_GET_ID_INFO_V2_PARAMS, NV0000_CTRL_GPU_GET_PCI_INFO_PARAMS,
    NV0000_CTRL_GPU_INVALID_ID, NV0000_CTRL_GPU_MAX_ATTACHED_GPUS, NvHandle,
};
use crate::server::gpu_controller::PciSlotInfo;
use anyhow::{bail, ensure};

pub(super) unsafe fn query_gpu_instances(
    fd: RawFd,
    client: NvHandle,
    pci: &PciSlotInfo,
) -> anyhow::Result<(u32, u32)> {
    unsafe {
        resolve_gpu_instance(
            pci,
            |params| {
                rm_control(
                    fd,
                    client,
                    client,
                    NV0000_CTRL_CMD_GPU_GET_ATTACHED_IDS,
                    params,
                )
            },
            |params| rm_control(fd, client, client, NV0000_CTRL_CMD_GPU_GET_PCI_INFO, params),
            |params| {
                rm_control(
                    fd,
                    client,
                    client,
                    NV0000_CTRL_CMD_GPU_GET_ID_INFO_V2,
                    params,
                )
            },
        )
    }
}

// The RM instances for NV0080/NV2080 can have a different order than Linux
// device minors. Resolve them by matching the PCI identity against RM.
fn resolve_gpu_instance(
    pci: &PciSlotInfo,
    get_attached_ids: impl FnOnce(&mut NV0000_CTRL_GPU_GET_ATTACHED_IDS_PARAMS) -> anyhow::Result<()>,
    mut get_pci_info: impl FnMut(&mut NV0000_CTRL_GPU_GET_PCI_INFO_PARAMS) -> anyhow::Result<()>,
    get_id_info: impl FnOnce(&mut NV0000_CTRL_GPU_GET_ID_INFO_V2_PARAMS) -> anyhow::Result<()>,
) -> anyhow::Result<(u32, u32)> {
    // The RM query exposes domain/bus/slot, but no PCI function.
    ensure!(pci.func == 0, "RM GPU lookup requires PCI function zero");
    let mut attached = NV0000_CTRL_GPU_GET_ATTACHED_IDS_PARAMS {
        gpuIds: [NV0000_CTRL_GPU_INVALID_ID; NV0000_CTRL_GPU_MAX_ATTACHED_GPUS as usize],
    };
    get_attached_ids(&mut attached)?;

    for gpu_id in attached.gpuIds {
        if gpu_id == NV0000_CTRL_GPU_INVALID_ID {
            continue;
        }

        let mut location = NV0000_CTRL_GPU_GET_PCI_INFO_PARAMS {
            gpuId: gpu_id,
            domain: 0,
            bus: 0,
            slot: 0,
        };
        get_pci_info(&mut location)?;
        if (location.domain, location.bus, location.slot)
            != (u32::from(pci.domain), pci.bus, pci.dev)
        {
            continue;
        }

        let mut info: NV0000_CTRL_GPU_GET_ID_INFO_V2_PARAMS = unsafe { mem::zeroed() };
        info.gpuId = gpu_id;
        get_id_info(&mut info)?;
        return Ok((info.deviceInstance, info.subDeviceInstance));
    }

    bail!("No Nvidia RM GPU matches PCI location {pci:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_pci_identity_when_minor_and_rm_orders_differ() {
        // IDs are opaque and enumeration order must not select the device.
        let pci = PciSlotInfo {
            domain: 0,
            bus: 0x0d,
            dev: 0,
            func: 0,
        };
        let instances = resolve_gpu_instance(
            &pci,
            |attached| {
                attached.gpuIds[0] = 0x2e00;
                attached.gpuIds[2] = 0x0d00;
                Ok(())
            },
            |location| {
                location.bus = match location.gpuId {
                    0x2e00 => 0x2e,
                    0x0d00 => 0x0d,
                    id => panic!("unexpected GPU ID {id:#x}"),
                };
                Ok(())
            },
            |info| {
                assert_eq!(info.gpuId, 0x0d00);
                info.deviceInstance = 5;
                info.subDeviceInstance = 2;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(instances, (5, 2));
    }

    #[test]
    fn does_not_fall_back_to_another_gpu_when_pci_is_missing() {
        let pci = PciSlotInfo {
            domain: 1,
            bus: 0x0d,
            dev: 0,
            func: 0,
        };
        let result = resolve_gpu_instance(
            &pci,
            |attached| {
                attached.gpuIds[0] = 0x0d00;
                Ok(())
            },
            |location| {
                location.bus = 0x0d;
                Ok(())
            },
            |_| panic!("must not allocate a GPU from another PCI domain"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn propagates_rm_query_failure() {
        let pci = PciSlotInfo {
            domain: 0,
            bus: 0x0d,
            dev: 0,
            func: 0,
        };
        let error = resolve_gpu_instance(
            &pci,
            |attached| {
                attached.gpuIds[0] = 0x2e00;
                attached.gpuIds[1] = 0x0d00;
                Ok(())
            },
            |location| {
                assert_eq!(location.gpuId, 0x2e00);
                bail!("RM unavailable");
            },
            |_| panic!("must not continue after an RM query fails"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("RM unavailable"));
    }

    #[test]
    fn rejects_nonzero_pci_function_before_querying_rm() {
        let pci = PciSlotInfo {
            domain: 0,
            bus: 0x0d,
            dev: 0,
            func: 1,
        };
        let result = resolve_gpu_instance(
            &pci,
            |_| panic!("must not query attached IDs for a nonzero function"),
            |_| panic!("must not query PCI info for a nonzero function"),
            |_| panic!("must not query instance IDs for a nonzero function"),
        );
        assert!(result.is_err());
    }
}
