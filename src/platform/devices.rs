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
        Ok(())
    }
}
fn changed() -> windows::core::Result<()> {
    CHANGED.store(true, Ordering::Release);
    Ok(())
}
fn take_changed() -> bool {
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

type DeviceLists = (Vec<String>, Vec<String>);
pub fn refresh() -> Option<DeviceLists> {
    static RESULTS: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Receiver<DeviceLists>>> =
        std::sync::OnceLock::new();
    let receiver = RESULTS.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            loop {
                if take_changed()
                    && tx
                        .send((
                            crate::audio::input_devices(),
                            crate::call_capture::outputs(),
                        ))
                        .is_err()
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
        std::sync::Mutex::new(rx)
    });
    receiver
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .try_recv()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn property_notifications_do_not_request_enumeration() {
        let listener: IMMNotificationClient = Listener.into();
        CHANGED.store(false, Ordering::Release);
        unsafe {
            listener
                .OnPropertyValueChanged(PCWSTR::null(), PROPERTYKEY::default())
                .unwrap();
        }
        assert!(!CHANGED.load(Ordering::Acquire));
        unsafe {
            listener.OnDeviceAdded(PCWSTR::null()).unwrap();
        }
        assert!(CHANGED.swap(false, Ordering::AcqRel));
    }
}
