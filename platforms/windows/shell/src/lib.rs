//! Explorer shell extension: adds "Copy here / Move here / Paste with Bifrost Drive" to the
//! right-drag and context menus for files going to or from a Bifrost drive, and hands the job to
//! the desktop app's transfer window. Windows only; on other hosts this crate is empty.
#![cfg(target_os = "windows")]

mod com;
pub mod decision;
pub mod registration;
pub mod request;
pub mod setting;
pub mod volume;

pub use com::{
    DllCanUnloadNow, DllGetClassObject, DllMain, DllRegisterServer, DllUnregisterServer, CLSID_MENU,
};
