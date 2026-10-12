use std::mem::MaybeUninit;

use anyhow::{ensure, Context};
use coreaudio::AppTapAggregateDevice;
use cpal::traits::HostTrait;
use objc2::AnyThread;
use objc2_core_audio::{
    AudioHardwareCreateProcessTap, AudioHardwareDestroyProcessTap, AudioObjectID, CATapDescription,
    CATapMuteBehavior,
};
use objc2_foundation::{NSArray, NSString};

struct ProcessTap(AudioObjectID);

impl Drop for ProcessTap {
    fn drop(&mut self) {
        // SAFETY: This handle owns the tap returned by AudioHardwareCreateProcessTap.
        let status = unsafe { AudioHardwareDestroyProcessTap(self.0) };
        if status != 0 {
            log::error!("Failed to destroy desktop audio tap: OSStatus {status}");
        }
    }
}

/// Owns the private aggregate device and its system-wide stereo tap.
/// The aggregate is destroyed before its tap, including on stream setup failure.
pub(super) struct DesktopTap {
    device: cpal::Device,
    _aggregate: AppTapAggregateDevice,
    _process_tap: ProcessTap,
}

impl DesktopTap {
    pub(super) fn new() -> anyhow::Result<Self> {
        // SAFETY: The empty exclusion list captures all processes. The retained
        // description remains alive throughout tap creation and UUID retrieval.
        let description = unsafe {
            let description = CATapDescription::initStereoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &NSArray::new(),
            );
            description.setName(&NSString::from_str("VU desktop audio"));
            description.setPrivate(true);
            description.setMuteBehavior(CATapMuteBehavior::Unmuted);
            description
        };

        let mut tap_id = MaybeUninit::uninit();
        // SAFETY: tap_id is writable; Core Audio initializes it on success.
        let status =
            unsafe { AudioHardwareCreateProcessTap(Some(&description), tap_id.as_mut_ptr()) };
        ensure!(
            status == 0,
            "Creating desktop audio tap failed: OSStatus {status}"
        );
        // SAFETY: A successful creation initialized the handle.
        let process_tap = ProcessTap(unsafe { tap_id.assume_init() });

        // SAFETY: The live tap description supplies its retained UUID string.
        let tap_uid = unsafe { description.UUID().UUIDString() }.to_string();
        let device_uid = format!("com.zezic.vu.tap.{tap_uid}");
        let aggregate = AppTapAggregateDevice::with_tap("VU desktop audio", &device_uid, &tap_uid)?;
        let device = cpal::default_host()
            .device_by_id(&cpal::DeviceId::new(cpal::HostId::CoreAudio, device_uid))
            .context("The private desktop audio device is unavailable")?;
        Ok(Self {
            device,
            _aggregate: aggregate,
            _process_tap: process_tap,
        })
    }

    pub(super) fn device(&self) -> cpal::Device {
        self.device.clone()
    }
}
