//! Mirrors Services/AudioDeviceWatcher.cs and MMDevice enumeration in WasapiAudioSource.cs.
use crate::{
    wasapi::{enumerator, Apartment},
    Result,
};
use std::{sync::Arc, thread::JoinHandle};
use windows::{
    core::{implement, PCWSTR},
    Win32::{
        Foundation::PROPERTYKEY,
        Media::Audio::*,
        System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc},
        System::Com::{CoTaskMemFree, STGM_READ},
    },
};
#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_capture: bool,
}
pub(crate) unsafe fn friendly_name(d: &IMMDevice) -> String {
    let get = || -> windows::core::Result<String> {
        let store = d.OpenPropertyStore(STGM_READ)?;
        let key =
            PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };
        let mut value = store.GetValue(&key)?;
        let result = PropVariantToStringAlloc(&value).map(|s| {
            let text = s.to_string().unwrap_or_default();
            CoTaskMemFree(Some(s.0.cast()));
            text
        });
        let _ = PropVariantClear(&mut value);
        result
    };
    get().unwrap_or_default()
}
pub fn enumerate_devices() -> Result<Vec<AudioDevice>> {
    let _com = Apartment::mta()?;
    let en = enumerator()?;
    let mut out = Vec::new();
    unsafe {
        for (flow, is_capture) in [(eRender, false), (eCapture, true)] {
            let collection = en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
            for i in 0..collection.GetCount()? {
                let d = collection.Item(i)?;
                let id = d.GetId()?;
                let s = id.to_string().unwrap_or_default();
                CoTaskMemFree(Some(id.0.cast()));
                out.push(AudioDevice { id: s, name: friendly_name(&d), is_capture });
            }
        }
    }
    Ok(out)
}
#[implement(IMMNotificationClient)]
struct Notification(Arc<dyn Fn() + Send + Sync>);
impl Notification_Impl {
    fn raise(&self) -> windows::core::Result<()> {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.0)()));
        Ok(())
    }
}
impl IMMNotificationClient_Impl for Notification_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows::core::Result<()> {
        self.raise()
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> windows::core::Result<()> {
        self.raise()
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows::core::Result<()> {
        self.raise()
    }
    fn OnDefaultDeviceChanged(&self, _: EDataFlow, _: ERole, _: &PCWSTR) -> windows::core::Result<()> {
        self.raise()
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}
pub struct AudioDeviceWatcher {
    stop: crossbeam_channel::Sender<()>,
    worker: Option<JoinHandle<()>>,
}
impl AudioDeviceWatcher {
    /// Callback runs on a system COM thread. Marshal/debounce in the UI subscriber.
    pub fn new(callback: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        let (stop, rx) = crossbeam_channel::bounded(1);
        let (ready, wait) = crossbeam_channel::bounded(1);
        let worker = std::thread::Builder::new().name("AudioDeviceWatcher".into()).spawn(move || {
            let run = || -> Result<()> {
                let _com = Apartment::mta()?;
                let en = enumerator()?;
                let handler: IMMNotificationClient = Notification(Arc::new(callback)).into();
                unsafe {
                    en.RegisterEndpointNotificationCallback(&handler)?;
                }
                let _ = ready.send(Ok(()));
                let _ = rx.recv();
                unsafe {
                    let _ = en.UnregisterEndpointNotificationCallback(&handler);
                }
                Ok(())
            };
            if let Err(e) = run() {
                log::warn!("[AudioDeviceWatcher] register failed: {e}");
                let _ = ready.send(Err(e));
            }
        })?;
        match wait.recv() {
            Ok(Ok(())) => Ok(Self { stop, worker: Some(worker) }),
            r => {
                let _ = worker.join();
                Err(match r {
                    Ok(Err(e)) => e,
                    _ => crate::AudioError("Watcher thread exited".into()),
                })
            }
        }
    }
}
impl Drop for AudioDeviceWatcher {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(t) = self.worker.take() {
            let _ = t.join();
        }
    }
}
