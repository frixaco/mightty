//! Windows default-terminal COM handoff.
//!
//! Windows launches the registered local COM server with `-Embedding`. The
//! `ITerminalHandoff3` callback supplies an existing console session. This
//! module duplicates its borrowed handles before the COM call returns.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::{FromRawHandle, IntoRawHandle, OwnedHandle as WindowsOwnedHandle};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, E_FAIL, E_NOINTERFACE, E_POINTER, HANDLE,
    INVALID_HANDLE_VALUE, SysStringLen,
};
use windows_sys::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoInitializeEx, CoRegisterClassObject,
    CoRevokeClassObject, CoUninitialize, REGCLS_MULTIPLEUSE,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOW, SetForegroundWindow, ShowWindow};
use windows_sys::core::{GUID, HRESULT, IID_IUnknown, IUnknown_Vtbl};

use crate::shell::PtyParts;

/// CLSID advertised by the packaged mightty terminal-host extension.
pub const CLSID_MIGHTTY_TERMINAL_HOST: GUID =
    GUID::from_u128(0xd4725759_69bd_469f_9819_f27e6c135ed5);

const IID_ICLASS_FACTORY: GUID = GUID::from_u128(0x00000001_0000_0000_c000_000000000046);
const IID_ITERMINAL_HANDOFF3: GUID = GUID::from_u128(0x6f23da90_15c5_4203_9db0_64e73f1b1b00);
const CLASS_E_NOAGGREGATION: HRESULT = 0x8004_0110u32 as HRESULT;
const E_UNEXPECTED: HRESULT = 0x8000_ffffu32 as HRESULT;
const MAX_STARTUP_TITLE_UNITS: u32 = 32 * 1024;

/// One console session received through `ITerminalHandoff3`.
pub struct DefaultTerminalHandoff {
    parts: PtyParts,
    startup_title: Option<String>,
    accepted: mpsc::SyncSender<bool>,
}

impl DefaultTerminalHandoff {
    /// Split the request into UI data and its COM response.
    pub fn into_parts(self) -> (PtyParts, Option<String>, DefaultTerminalResponse) {
        (
            self.parts,
            self.startup_title,
            DefaultTerminalResponse {
                accepted: self.accepted,
            },
        )
    }
}

/// Completes the blocked COM handoff after the UI accepts or rejects it.
pub struct DefaultTerminalResponse {
    accepted: mpsc::SyncSender<bool>,
}

impl DefaultTerminalResponse {
    pub fn send(self, accepted: bool) {
        let _ = self.accepted.send(accepted);
    }
}

/// Owns the local COM class registration and its worker thread.
pub struct DefaultTerminalServer {
    stop_tx: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl DefaultTerminalServer {
    /// Register the mightty terminal host and forward handoffs to `sender`.
    pub fn start(sender: flume::Sender<DefaultTerminalHandoff>) -> io::Result<Self> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (stop_tx, stop_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("mightty-terminal-handoff".to_string())
            .spawn(move || terminal_server_thread(sender, ready_tx, stop_rx))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop_tx: Some(stop_tx),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(io::Error::other(
                    "default-terminal COM server stopped during startup",
                ))
            }
        }
    }
}

impl Drop for DefaultTerminalServer {
    fn drop(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Show and activate the normal window after a hidden COM start.
pub fn show_default_terminal_window(window: &mut Window) -> io::Result<()> {
    let raw = window
        .window_handle()
        .map_err(|error| io::Error::other(error.to_string()))?
        .as_raw();
    let RawWindowHandle::Win32(handle) = raw else {
        return Err(io::Error::other("GPUI did not provide a Windows handle"));
    };
    let hwnd = handle.hwnd.get() as HANDLE;
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    window.activate_window();
    Ok(())
}

fn terminal_server_thread(
    sender: flume::Sender<DefaultTerminalHandoff>,
    ready: mpsc::SyncSender<io::Result<()>>,
    stop: mpsc::Receiver<()>,
) {
    let initialize_result = unsafe { CoInitializeEx(null(), COINIT_MULTITHREADED as u32) };
    if initialize_result < 0 {
        let _ = ready.send(Err(hresult_error(
            "initialize default-terminal COM",
            initialize_result,
        )));
        return;
    }

    let factory = Box::into_raw(Box::new(ClassFactory {
        vtable: &CLASS_FACTORY_VTABLE,
        references: AtomicU32::new(1),
        sender,
    }));
    let mut registration = 0;
    let register_result = unsafe {
        CoRegisterClassObject(
            &CLSID_MIGHTTY_TERMINAL_HOST,
            factory.cast(),
            CLSCTX_LOCAL_SERVER,
            REGCLS_MULTIPLEUSE as u32,
            &mut registration,
        )
    };
    if register_result < 0 {
        unsafe {
            class_factory_release(factory.cast());
            CoUninitialize();
        }
        let _ = ready.send(Err(hresult_error(
            "register default-terminal COM class",
            register_result,
        )));
        return;
    }

    if ready.send(Ok(())).is_ok() {
        let _ = stop.recv();
    }

    unsafe {
        let _ = CoRevokeClassObject(registration);
        class_factory_release(factory.cast());
        CoUninitialize();
    }
}

#[repr(C)]
struct ClassFactory {
    vtable: *const ClassFactoryVtable,
    references: AtomicU32,
    sender: flume::Sender<DefaultTerminalHandoff>,
}

#[repr(C)]
struct ClassFactoryVtable {
    base: IUnknown_Vtbl,
    create_instance: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const GUID,
        *mut *mut c_void,
    ) -> HRESULT,
    lock_server: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
}

static CLASS_FACTORY_VTABLE: ClassFactoryVtable = ClassFactoryVtable {
    base: IUnknown_Vtbl {
        QueryInterface: class_factory_query_interface,
        AddRef: class_factory_add_ref,
        Release: class_factory_release,
    },
    create_instance: class_factory_create_instance,
    lock_server: class_factory_lock_server,
};

unsafe extern "system" fn class_factory_query_interface(
    this: *mut c_void,
    iid: *const GUID,
    interface: *mut *mut c_void,
) -> HRESULT {
    if this.is_null() || iid.is_null() || interface.is_null() {
        return E_POINTER;
    }
    unsafe {
        *interface = null_mut();
        if guid_eq(&*iid, &IID_IUnknown) || guid_eq(&*iid, &IID_ICLASS_FACTORY) {
            *interface = this;
            class_factory_add_ref(this);
            return 0;
        }
    }
    E_NOINTERFACE
}

unsafe extern "system" fn class_factory_add_ref(this: *mut c_void) -> u32 {
    if this.is_null() {
        return 0;
    }
    unsafe {
        (*this.cast::<ClassFactory>())
            .references
            .fetch_add(1, Ordering::Relaxed)
            + 1
    }
}

unsafe extern "system" fn class_factory_release(this: *mut c_void) -> u32 {
    if this.is_null() {
        return 0;
    }
    let factory = this.cast::<ClassFactory>();
    let previous = unsafe { (*factory).references.fetch_sub(1, Ordering::Release) };
    if previous == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            drop(Box::from_raw(factory));
        }
        0
    } else {
        previous - 1
    }
}

unsafe extern "system" fn class_factory_create_instance(
    this: *mut c_void,
    outer: *mut c_void,
    iid: *const GUID,
    interface: *mut *mut c_void,
) -> HRESULT {
    if this.is_null() || iid.is_null() || interface.is_null() {
        return E_POINTER;
    }
    unsafe {
        *interface = null_mut();
    }
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }

    catch_unwind(AssertUnwindSafe(|| unsafe {
        let sender = (*this.cast::<ClassFactory>()).sender.clone();
        let object = Box::into_raw(Box::new(TerminalHandoffObject {
            vtable: &TERMINAL_HANDOFF_VTABLE,
            references: AtomicU32::new(1),
            sender,
        }));
        let result = terminal_handoff_query_interface(object.cast(), iid, interface);
        terminal_handoff_release(object.cast());
        result
    }))
    .unwrap_or(E_UNEXPECTED)
}

unsafe extern "system" fn class_factory_lock_server(_: *mut c_void, _: i32) -> HRESULT {
    0
}

#[repr(C)]
struct TerminalHandoffObject {
    vtable: *const TerminalHandoffVtable,
    references: AtomicU32,
    sender: flume::Sender<DefaultTerminalHandoff>,
}

#[repr(C)]
struct TerminalHandoffVtable {
    base: IUnknown_Vtbl,
    establish_pty_handoff: unsafe extern "system" fn(
        *mut c_void,
        *mut HANDLE,
        *mut HANDLE,
        HANDLE,
        HANDLE,
        HANDLE,
        HANDLE,
        *const TerminalStartupInfo,
    ) -> HRESULT,
}

#[repr(C)]
struct TerminalStartupInfo {
    title: *const u16,
    icon_path: *const u16,
    icon_index: i32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
    fill_attribute: u32,
    flags: u32,
    show_window: u16,
}

static TERMINAL_HANDOFF_VTABLE: TerminalHandoffVtable = TerminalHandoffVtable {
    base: IUnknown_Vtbl {
        QueryInterface: terminal_handoff_query_interface,
        AddRef: terminal_handoff_add_ref,
        Release: terminal_handoff_release,
    },
    establish_pty_handoff,
};

unsafe extern "system" fn terminal_handoff_query_interface(
    this: *mut c_void,
    iid: *const GUID,
    interface: *mut *mut c_void,
) -> HRESULT {
    if this.is_null() || iid.is_null() || interface.is_null() {
        return E_POINTER;
    }
    unsafe {
        *interface = null_mut();
        if guid_eq(&*iid, &IID_IUnknown) || guid_eq(&*iid, &IID_ITERMINAL_HANDOFF3) {
            *interface = this;
            terminal_handoff_add_ref(this);
            return 0;
        }
    }
    E_NOINTERFACE
}

unsafe extern "system" fn terminal_handoff_add_ref(this: *mut c_void) -> u32 {
    if this.is_null() {
        return 0;
    }
    unsafe {
        (*this.cast::<TerminalHandoffObject>())
            .references
            .fetch_add(1, Ordering::Relaxed)
            + 1
    }
}

unsafe extern "system" fn terminal_handoff_release(this: *mut c_void) -> u32 {
    if this.is_null() {
        return 0;
    }
    let object = this.cast::<TerminalHandoffObject>();
    let previous = unsafe { (*object).references.fetch_sub(1, Ordering::Release) };
    if previous == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        unsafe {
            drop(Box::from_raw(object));
        }
        0
    } else {
        previous - 1
    }
}

unsafe extern "system" fn establish_pty_handoff(
    this: *mut c_void,
    input: *mut HANDLE,
    output: *mut HANDLE,
    signal: HANDLE,
    reference: HANDLE,
    server: HANDLE,
    client: HANDLE,
    startup_info: *const TerminalStartupInfo,
) -> HRESULT {
    if this.is_null() || input.is_null() || output.is_null() {
        return E_POINTER;
    }
    unsafe {
        *input = null_mut();
        *output = null_mut();
    }

    catch_unwind(AssertUnwindSafe(|| {
        establish_pty_handoff_inner(
            this,
            HandoffCall {
                input,
                output,
                signal,
                reference,
                server,
                client,
                startup_info,
            },
        )
    }))
    .unwrap_or(E_UNEXPECTED)
}

struct HandoffCall {
    input: *mut HANDLE,
    output: *mut HANDLE,
    signal: HANDLE,
    reference: HANDLE,
    server: HANDLE,
    client: HANDLE,
    startup_info: *const TerminalStartupInfo,
}

fn establish_pty_handoff_inner(this: *mut c_void, call: HandoffCall) -> HRESULT {
    let signal = match unsafe { duplicate_handle(call.signal) } {
        Ok(handle) => handle,
        Err(error) => return hresult_from_io(&error),
    };
    let reference = match unsafe { duplicate_handle(call.reference) } {
        Ok(handle) => handle,
        Err(error) => return hresult_from_io(&error),
    };
    let server = match unsafe { duplicate_handle(call.server) } {
        Ok(handle) => handle,
        Err(error) => return hresult_from_io(&error),
    };
    let client = match unsafe { duplicate_handle(call.client) } {
        Ok(handle) => handle,
        Err(error) => return hresult_from_io(&error),
    };
    let handoff = match PtyParts::from_handoff(signal, reference, server, client) {
        Ok(handoff) => handoff,
        Err(_) => return E_FAIL,
    };
    let (parts, input_peer, output_peer) = handoff.into_parts();
    let startup_title = unsafe { startup_title(call.startup_info) };
    let (accepted_tx, accepted_rx) = mpsc::sync_channel(1);
    let request = DefaultTerminalHandoff {
        parts,
        startup_title,
        accepted: accepted_tx,
    };
    let sender = unsafe { &(*this.cast::<TerminalHandoffObject>()).sender };
    if sender.send(request).is_err() || accepted_rx.recv().ok() != Some(true) {
        return E_FAIL;
    }

    unsafe {
        *call.input = input_peer.into_raw_handle();
        *call.output = output_peer.into_raw_handle();
    }
    0
}

unsafe fn duplicate_handle(handle: HANDLE) -> io::Result<WindowsOwnedHandle> {
    let process = unsafe { GetCurrentProcess() };
    let mut duplicate = INVALID_HANDLE_VALUE;
    let result = unsafe {
        DuplicateHandle(
            process,
            handle,
            process,
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(unsafe { WindowsOwnedHandle::from_raw_handle(duplicate) })
}

unsafe fn startup_title(info: *const TerminalStartupInfo) -> Option<String> {
    let title = unsafe { info.as_ref()?.title };
    if title.is_null() {
        return None;
    }
    let length = unsafe { SysStringLen(title) };
    if length == 0 || length > MAX_STARTUP_TITLE_UNITS {
        return None;
    }
    let units = unsafe { std::slice::from_raw_parts(title, length as usize) };
    Some(String::from_utf16_lossy(units))
}

fn guid_eq(left: &GUID, right: &GUID) -> bool {
    left.data1 == right.data1
        && left.data2 == right.data2
        && left.data3 == right.data3
        && left.data4 == right.data4
}

fn hresult_from_io(error: &io::Error) -> HRESULT {
    let code = error.raw_os_error().unwrap_or(1).max(1) as u32;
    (0x8007_0000u32 | (code & 0xffff)) as HRESULT
}

fn hresult_error(operation: &str, result: HRESULT) -> io::Error {
    io::Error::other(format!("{operation} failed with HRESULT 0x{result:08X}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_windows_terminal_handoff_contract_ids() {
        assert!(guid_eq(
            &IID_ITERMINAL_HANDOFF3,
            &GUID::from_u128(0x6f23da90_15c5_4203_9db0_64e73f1b1b00)
        ));
        assert!(guid_eq(
            &CLSID_MIGHTTY_TERMINAL_HOST,
            &GUID::from_u128(0xd4725759_69bd_469f_9819_f27e6c135ed5)
        ));
    }

    #[test]
    fn class_factory_rejects_aggregation() {
        let (sender, _receiver) = flume::unbounded();
        let factory = Box::into_raw(Box::new(ClassFactory {
            vtable: &CLASS_FACTORY_VTABLE,
            references: AtomicU32::new(1),
            sender,
        }));
        let mut interface = null_mut();
        let result = unsafe {
            class_factory_create_instance(
                factory.cast(),
                std::ptr::dangling_mut::<c_void>(),
                &IID_ITERMINAL_HANDOFF3,
                &mut interface,
            )
        };
        assert_eq!(result, CLASS_E_NOAGGREGATION);
        assert!(interface.is_null());
        unsafe {
            class_factory_release(factory.cast());
        }
    }

    #[test]
    fn terminal_object_supports_only_its_public_interface() {
        let (sender, _receiver) = flume::unbounded();
        let object = Box::into_raw(Box::new(TerminalHandoffObject {
            vtable: &TERMINAL_HANDOFF_VTABLE,
            references: AtomicU32::new(1),
            sender,
        }));
        let mut interface = null_mut();
        assert_eq!(
            unsafe {
                terminal_handoff_query_interface(
                    object.cast(),
                    &IID_ITERMINAL_HANDOFF3,
                    &mut interface,
                )
            },
            0
        );
        assert_eq!(interface, object.cast());
        unsafe {
            terminal_handoff_release(interface);
            terminal_handoff_release(object.cast());
        }
    }

    #[test]
    fn registers_and_revokes_local_com_server() {
        let (sender, _receiver) = flume::unbounded();
        let server = DefaultTerminalServer::start(sender)
            .expect("register local default-terminal COM server");
        drop(server);
    }
}
