//! Process-lifetime endpoint notifications. Callbacks never enumerate devices.
use std::sync::{
    Once,
    atomic::{AtomicBool, Ordering},
};
use windows::{
    Win32::{Foundation::PROPERTYKEY, Media::Audio::*, System::Com::*},
    core::{PCWSTR, implement},
};
static CHANGED: AtomicBool = AtomicBool::new(false);
static START: Once = Once::new();
#[implement(IMMNotificationClient)]
struct Listener;
#[allow(non_snake_case)]
impl IMMNotificationClient_Impl for Listener_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows::core::Result<()> {
        changed()
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> windows::core::Result<()> {
        changed()
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows::core::Result<()> {
        changed()
    }
    fn OnDefaultDeviceChanged(
        &self,
        _: EDataFlow,
        _: ERole,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        changed()
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> windows::core::Result<()> {
        changed()
    }
}
fn changed() -> windows::core::Result<()> {
    CHANGED.store(true, Ordering::Release);
    Ok(())
}
pub fn take_changed() -> bool {
    START.call_once(|| {
        std::thread::spawn(|| unsafe {
            if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                return;
            }
            let result = (|| -> windows::core::Result<()> {
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                let listener: IMMNotificationClient = Listener.into();
                enumerator.RegisterEndpointNotificationCallback(&listener)?;
                loop {
                    std::thread::park();
                }
            })();
            if let Err(error) = result {
                eprintln!("Audio device notifications unavailable: {error}");
            }
            CoUninitialize();
        });
    });
    CHANGED.swap(false, Ordering::AcqRel)
}
