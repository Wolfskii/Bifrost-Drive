//! The Explorer-facing COM object. It only decides whether to show menu items and, when one
//! is chosen, writes a request file and starts Bifrost Drive. It runs inside explorer.exe, so
//! it does no transfer work itself and never lets a panic escape.

use std::{
    cell::{Cell, RefCell},
    ffi::{c_void, CStr, OsString},
    os::windows::ffi::OsStringExt,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};

use windows::{
    core::{
        implement, w, Error, IUnknown, Interface, Ref, Result, BOOL, GUID, HRESULT, PCWSTR, PSTR,
    },
    Win32::{
        Foundation::{
            CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_NOTIMPL, E_POINTER,
            E_UNEXPECTED, HINSTANCE, HMODULE, HWND, S_FALSE, S_OK, TRUE,
        },
        System::{
            Com::{
                IClassFactory, IClassFactory_Impl, IDataObject, DVASPECT_CONTENT, FORMATETC,
                TYMED_HGLOBAL,
            },
            DataExchange::RegisterClipboardFormatW,
            LibraryLoader::{DisableThreadLibraryCalls, GetModuleFileNameW},
            Memory::{GlobalLock, GlobalUnlock},
            Ole::{OleGetClipboard, OleSetClipboard, ReleaseStgMedium, CF_HDROP, DROPEFFECT_MOVE},
            Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
        },
        UI::{
            Shell::{
                Common::ITEMIDLIST, DragQueryFileW, IContextMenu, IContextMenu_Impl, IShellExtInit,
                IShellExtInit_Impl, SHChangeNotify, SHGetPathFromIDListW, CMF_DEFAULTONLY,
                CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, GCS_HELPTEXTW, GCS_VERBW, HDROP,
                SHCNE_ASSOCCHANGED, SHCNF_IDLIST,
            },
            WindowsAndMessaging::{
                InsertMenuW, MessageBoxW, HMENU, IDYES, MB_DEFBUTTON2, MB_ICONERROR,
                MB_ICONWARNING, MB_OK, MB_YESNO, MF_BYPOSITION, MF_STRING,
            },
        },
    },
};

use crate::{
    decision::{self, Gesture, MenuEntry, Mode},
    registration, request, setting, volume,
};

/// Identifies the menu handler to COM and in the registry.
pub const CLSID_MENU: GUID = GUID::from_u128(0x7a3c1f52_5b0e_4d7b_9e34_2c1a6f8b90d1);

static INSTANCE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static OBJECTS: AtomicUsize = AtomicUsize::new(0);
static LOCKS: AtomicUsize = AtomicUsize::new(0);

const APP_EXE_FALLBACK: &str = "bifrost-drive.exe";
/// `CMINVOKECOMMANDINFOEX.lpVerbW` and friends are valid (the SDK's CMIC_MASK_UNICODE).
const CMIC_MASK_UNICODE: u32 = 0x0000_4000;
const MAX_PATH_UNITS: usize = 32_768;

/// Runs `body`, turning a panic into a COM error instead of unwinding into Explorer.
fn guarded<T>(on_panic: T, body: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(on_panic)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn path_from_pidl(pidl: *const ITEMIDLIST) -> Option<PathBuf> {
    if pidl.is_null() {
        return None;
    }
    let mut buffer = [0u16; 260];
    unsafe { SHGetPathFromIDListW(pidl, &mut buffer) }
        .as_bool()
        .then(|| {
            let length = buffer.iter().position(|&unit| unit == 0).unwrap_or(260);
            PathBuf::from(OsString::from_wide(&buffer[..length]))
        })
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

/// The file paths in a data object (an Explorer selection, a drop, or the clipboard).
fn files_in(data: &IDataObject) -> Result<Vec<PathBuf>> {
    let format = hdrop_format();
    let mut medium = unsafe { data.GetData(&format)? };
    let hdrop = HDROP(unsafe { medium.u.hGlobal.0 });
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    let mut files = Vec::with_capacity(count as usize);
    for index in 0..count {
        let length = unsafe { DragQueryFileW(hdrop, index, None) } as usize;
        if length == 0 || length > MAX_PATH_UNITS {
            continue;
        }
        let mut buffer = vec![0u16; length + 1];
        let copied = unsafe { DragQueryFileW(hdrop, index, Some(&mut buffer)) } as usize;
        files.push(PathBuf::from(OsString::from_wide(&buffer[..copied])));
    }
    unsafe { ReleaseStgMedium(&mut medium) };
    Ok(files)
}

/// Whether the clipboard holds a cut (move) or a copy.
fn clipboard_mode(data: &IDataObject) -> Mode {
    let format = FORMATETC {
        cfFormat: unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) } as u16,
        ..hdrop_format()
    };
    let Ok(mut medium) = (unsafe { data.GetData(&format) }) else {
        return Mode::Copy;
    };
    let effect = unsafe {
        let pointer = GlobalLock(medium.u.hGlobal).cast::<u32>();
        let effect = if pointer.is_null() { 0 } else { *pointer };
        let _ = GlobalUnlock(medium.u.hGlobal);
        effect
    };
    unsafe { ReleaseStgMedium(&mut medium) };
    if effect & DROPEFFECT_MOVE.0 != 0 {
        Mode::Move
    } else {
        Mode::Copy
    }
}

/// Works out what the user is doing from what Explorer hands to `Initialize`.
fn gesture_for(
    folder: Option<PathBuf>,
    selection: Option<Vec<PathBuf>>,
    clipboard: impl FnOnce() -> Option<(Vec<PathBuf>, Mode)>,
    is_directory: impl Fn(&Path) -> bool,
) -> Vec<Gesture> {
    // A context menu on an item may also be given the folder that holds the item, which would
    // look like a drop of the item into its own folder. Nobody drops files where they already
    // are, so that combination is treated as a plain selection.
    let (folder, selection) = match (folder, selection) {
        (Some(folder), Some(selected))
            if selected
                .iter()
                .all(|item| item.parent() == Some(folder.as_path())) =>
        {
            (None, Some(selected))
        }
        other => other,
    };
    match (folder, selection) {
        // A right-drag: the dropped files and the folder they were dropped on.
        (Some(target), Some(sources)) => vec![Gesture::Drop { sources, target }],
        // Right-click on a folder's empty space.
        (Some(target), None) => clipboard()
            .map(|(sources, clipboard_mode)| Gesture::Paste {
                sources,
                target,
                clipboard_mode,
            })
            .into_iter()
            .collect(),
        // Right-click on items: they can be deleted, and a single folder or drive can also
        // receive what is on the clipboard.
        (None, Some(selected)) if !selected.is_empty() => {
            let mut readings = vec![Gesture::Selection {
                items: selected.clone(),
            }];
            if let [only] = selected.as_slice() {
                if is_directory(only) {
                    if let Some((sources, clipboard_mode)) = clipboard() {
                        readings.push(Gesture::Paste {
                            sources,
                            target: only.clone(),
                            clipboard_mode,
                        });
                    }
                }
            }
            readings
        }
        _ => Vec::new(),
    }
}

fn clipboard_files() -> Option<(Vec<PathBuf>, Mode)> {
    let data = unsafe { OleGetClipboard() }.ok()?;
    let files = files_in(&data).ok()?;
    (!files.is_empty()).then(|| (files, clipboard_mode(&data)))
}

#[implement(IShellExtInit, IContextMenu)]
pub struct MenuHandler {
    gestures: RefCell<Vec<Gesture>>,
    offered: RefCell<Vec<MenuEntry>>,
    first_command: Cell<u32>,
}

impl MenuHandler {
    pub fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self {
            gestures: RefCell::new(Vec::new()),
            offered: RefCell::new(Vec::new()),
            first_command: Cell::new(0),
        }
    }
}

impl Default for MenuHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MenuHandler {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IShellExtInit_Impl for MenuHandler_Impl {
    fn Initialize(
        &self,
        pidlfolder: *const ITEMIDLIST,
        pdtobj: Ref<IDataObject>,
        _hkeyprogid: HKEY,
    ) -> Result<()> {
        guarded(Err(Error::from(E_UNEXPECTED)), || {
            let selection = pdtobj.as_ref().and_then(|data| files_in(data).ok());
            let gestures = gesture_for(
                path_from_pidl(pidlfolder),
                selection,
                clipboard_files,
                Path::is_dir,
            );
            *self.gestures.borrow_mut() = gestures;
            Ok(())
        })
    }
}

impl IContextMenu_Impl for MenuHandler_Impl {
    fn QueryContextMenu(
        &self,
        hmenu: HMENU,
        indexmenu: u32,
        idcmdfirst: u32,
        idcmdlast: u32,
        uflags: u32,
    ) -> HRESULT {
        guarded(E_UNEXPECTED, || {
            if uflags & CMF_DEFAULTONLY != 0 {
                return HRESULT(0);
            }
            let gestures = self.gestures.borrow();
            if gestures.is_empty() {
                return HRESULT(0);
            }
            let room = idcmdlast.saturating_sub(idcmdfirst).saturating_add(1) as usize;
            let mut offered = decision::entries_for(&gestures, setting::menu_enabled(), &|path| {
                volume::is_bifrost_drive(path)
            });
            offered.truncate(room);
            for (position, entry) in offered.iter().enumerate() {
                let label = wide(entry.label);
                let inserted = unsafe {
                    InsertMenuW(
                        hmenu,
                        indexmenu + position as u32,
                        MF_BYPOSITION | MF_STRING,
                        (idcmdfirst + position as u32) as usize,
                        PCWSTR(label.as_ptr()),
                    )
                };
                if inserted.is_err() {
                    return E_FAIL;
                }
            }
            self.first_command.set(idcmdfirst);
            let count = offered.len() as i32;
            *self.offered.borrow_mut() = offered;
            // The shell reads the number of items added from the success code.
            HRESULT(count)
        })
    }

    fn InvokeCommand(&self, pici: *const CMINVOKECOMMANDINFO) -> Result<()> {
        guarded(Err(Error::from(E_UNEXPECTED)), || {
            if pici.is_null() {
                return Err(Error::from(E_POINTER));
            }
            let info = unsafe { &*pici };
            let entry = self.entry_for(info);
            let Some(entry) = entry else {
                return Err(Error::from(E_FAIL));
            };
            if entry.mode == Mode::Delete && !self.confirmed_delete(info.hwnd) {
                return Ok(());
            }
            match self.run(&entry) {
                Ok(()) => Ok(()),
                Err(message) => {
                    show_error(info.hwnd, &message);
                    Err(Error::from(E_FAIL))
                }
            }
        })
    }

    fn GetCommandString(
        &self,
        idcmd: usize,
        utype: u32,
        _preserved: *const u32,
        pszname: PSTR,
        cchmax: u32,
    ) -> Result<()> {
        guarded(Err(Error::from(E_UNEXPECTED)), || {
            let offered = self.offered.borrow();
            let Some(entry) = offered.get(idcmd) else {
                return Err(Error::from(E_NOTIMPL));
            };
            let text = match utype {
                GCS_VERBW => entry.verb,
                GCS_HELPTEXTW => entry.help,
                _ => return Err(Error::from(E_NOTIMPL)),
            };
            if pszname.is_null() || cchmax == 0 {
                return Err(Error::from(E_POINTER));
            }
            let text = wide(text);
            let length = text.len().min(cchmax as usize);
            let target = pszname.0.cast::<u16>();
            unsafe {
                std::ptr::copy_nonoverlapping(text.as_ptr(), target, length);
                *target.add(length - 1) = 0;
            }
            Ok(())
        })
    }
}

impl MenuHandler {
    /// The menu entry a command refers to: a position (from a click) or a verb name.
    fn entry_for(&self, info: &CMINVOKECOMMANDINFO) -> Option<MenuEntry> {
        let offered = self.offered.borrow();
        let unicode = (info.cbSize as usize) >= size_of::<CMINVOKECOMMANDINFOEX>()
            && info.fMask & CMIC_MASK_UNICODE != 0;
        if unicode {
            let extended =
                unsafe { &*(info as *const CMINVOKECOMMANDINFO).cast::<CMINVOKECOMMANDINFOEX>() };
            let verb = extended.lpVerbW.0 as usize;
            if verb <= 0xFFFF {
                return offered.get(verb).copied();
            }
            let name = unsafe { extended.lpVerbW.to_string() }.ok()?;
            return decision::entry_for_verb(&offered, &name);
        }
        let verb = info.lpVerb.0 as usize;
        if verb <= 0xFFFF {
            return offered.get(verb).copied();
        }
        let name = unsafe { CStr::from_ptr(info.lpVerb.0.cast()) }
            .to_str()
            .ok()?;
        decision::entry_for_verb(&offered, name)
    }

    /// Deleting cannot be undone, so it is the one action that asks first.
    fn confirmed_delete(&self, owner: HWND) -> bool {
        let gestures = self.gestures.borrow();
        let Some(Gesture::Selection { items }) = gestures
            .iter()
            .find(|gesture| matches!(gesture, Gesture::Selection { .. }))
        else {
            return false;
        };
        let text = wide(&decision::delete_prompt(items));
        let answer = unsafe {
            MessageBoxW(
                Some(owner),
                PCWSTR(text.as_ptr()),
                w!("Bifrost Drive"),
                MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
            )
        };
        answer == IDYES
    }

    fn run(&self, entry: &MenuEntry) -> std::result::Result<(), String> {
        let gestures = self.gestures.borrow();
        let gesture = decision::gesture_for_entry(&gestures, entry)
            .ok_or_else(|| "There is nothing selected to transfer".to_owned())?;
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| "Windows did not provide the app data folder".to_owned())?;
        perform(
            gesture,
            entry,
            &local_app_data,
            &app_exe(),
            &request::launch,
        )?;
        // Explorer empties the clipboard after a pasted cut; do the same so the files are
        // not offered again once they have moved.
        if matches!(
            gesture,
            Gesture::Paste {
                clipboard_mode: Mode::Move,
                ..
            }
        ) {
            let _ = unsafe { OleSetClipboard(None) };
        }
        Ok(())
    }
}

/// Saves the request and starts the app with it. A request that could not be handed over is
/// removed again so nothing stale is left for a later launch.
fn perform(
    gesture: &Gesture,
    entry: &MenuEntry,
    local_app_data: &Path,
    app: &Path,
    launch: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::result::Result<(), String> {
    let file = request::write_request(
        &request::requests_dir(local_app_data),
        &decision::request_for(gesture, entry),
    )
    .map_err(|error| format!("The transfer request could not be saved ({error})"))?;
    launch(app, &file).map_err(|error| {
        let _ = std::fs::remove_file(&file);
        format!(
            "Bifrost Drive could not be started from {} ({error})",
            app.display()
        )
    })
}

fn show_error(owner: HWND, message: &str) {
    let text = wide(message);
    unsafe {
        MessageBoxW(
            Some(owner),
            PCWSTR(text.as_ptr()),
            w!("Bifrost Drive"),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn module_path() -> Option<PathBuf> {
    let module = HMODULE(INSTANCE.load(Ordering::Relaxed));
    let mut buffer = vec![0u16; 1024];
    let length = unsafe { GetModuleFileNameW(Some(module), &mut buffer) } as usize;
    (length > 0 && length < buffer.len())
        .then(|| PathBuf::from(OsString::from_wide(&buffer[..length])))
}

/// Where the app lives: the installer records it, and a development build sits next to this DLL.
fn app_exe() -> PathBuf {
    let key = r"Software\Bifrost Drive";
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        if let Some(path) = registration::read_string(root, key, Some("ExePath")) {
            if Path::new(&path).is_file() {
                return PathBuf::from(path);
            }
        }
    }
    module_path()
        .and_then(|dll| dll.parent().map(|folder| folder.join(APP_EXE_FALLBACK)))
        .unwrap_or_else(|| PathBuf::from(APP_EXE_FALLBACK))
}

#[implement(IClassFactory)]
struct Factory;

impl Factory {
    fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for Factory {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        riid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        guarded(Err(Error::from(E_UNEXPECTED)), || {
            if object.is_null() || riid.is_null() {
                return Err(Error::from(E_POINTER));
            }
            unsafe { *object = std::ptr::null_mut() };
            if outer.is_some() {
                return Err(Error::from(CLASS_E_NOAGGREGATION));
            }
            let handler: IUnknown = MenuHandler::new().into();
            unsafe { handler.query(riid, object).ok() }
        })
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        if lock.as_bool() {
            LOCKS.fetch_add(1, Ordering::SeqCst);
        } else {
            LOCKS.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[no_mangle]
pub extern "system" fn DllMain(instance: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    const DLL_PROCESS_ATTACH: u32 = 1;
    if reason == DLL_PROCESS_ATTACH {
        INSTANCE.store(instance.0, Ordering::Relaxed);
        unsafe {
            let _ = DisableThreadLibraryCalls(HMODULE(instance.0));
        }
    }
    TRUE
}

/// # Safety
/// Called by COM with valid pointers or null.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    riid: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    guarded(E_UNEXPECTED, || {
        if clsid.is_null() || riid.is_null() || object.is_null() {
            return E_POINTER;
        }
        unsafe { *object = std::ptr::null_mut() };
        if unsafe { *clsid } != CLSID_MENU {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory::new().into();
        unsafe { factory.query(riid, object) }
    })
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECTS.load(Ordering::SeqCst) == 0 && LOCKS.load(Ordering::SeqCst) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

fn announce_association_change() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

/// Per-user registration for development: `regsvr32 bifrost_shell.dll`.
#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    guarded(E_UNEXPECTED, || {
        let Some(dll) = module_path() else {
            return E_FAIL;
        };
        match registration::register_under(
            HKEY_CURRENT_USER,
            r"Software\Classes",
            &dll.to_string_lossy(),
        ) {
            Ok(()) => {
                announce_association_change();
                S_OK
            }
            Err(error) => error.code(),
        }
    })
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    guarded(E_UNEXPECTED, || {
        match registration::unregister_under(HKEY_CURRENT_USER, r"Software\Classes") {
            Ok(()) => {
                announce_association_change();
                S_OK
            }
            Err(error) => error.code(),
        }
    })
}

#[cfg(test)]
mod tests {
    use windows::{
        core::{ComObject, PCSTR},
        Win32::{
            System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED},
            UI::{
                Shell::{IShellFolder, SHBindToParent, SHParseDisplayName},
                WindowsAndMessaging::{
                    CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuStringW,
                },
            },
        },
    };

    use super::*;

    struct Scenario {
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl Scenario {
        fn new(drive: char, menu_enabled: bool) -> Self {
            let guard = volume::TEST_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            volume::TEST_DRIVE.store(drive as u32, Ordering::Relaxed);
            setting::TEST_OVERRIDE.store(i32::from(menu_enabled), Ordering::Relaxed);
            Self { _guard: guard }
        }
    }

    impl Drop for Scenario {
        fn drop(&mut self) {
            volume::TEST_DRIVE.store(0, Ordering::Relaxed);
            setting::TEST_OVERRIDE.store(-1, Ordering::Relaxed);
        }
    }

    fn drop_onto_drive() -> Gesture {
        Gesture::Drop {
            sources: vec![PathBuf::from(r"C:\Videos\a.mkv")],
            target: PathBuf::from(r"Y:\Filmer"),
        }
    }

    fn handler_for(gesture: Gesture) -> (ComObject<MenuHandler>, IContextMenu) {
        let object = ComObject::new(MenuHandler::new());
        object.gestures.replace(vec![gesture]);
        let menu = object.to_interface::<IContextMenu>();
        (object, menu)
    }

    fn menu_texts(menu: HMENU) -> Vec<String> {
        (0..unsafe { GetMenuItemCount(Some(menu)) })
            .map(|position| {
                let mut text = [0u16; 96];
                let length = unsafe {
                    GetMenuStringW(menu, position as u32, Some(&mut text), MF_BYPOSITION)
                } as usize;
                String::from_utf16_lossy(&text[..length])
            })
            .collect()
    }

    #[test]
    fn the_class_factory_hands_out_a_handler_with_both_interfaces() {
        let _scenario = Scenario::new('Y', true);
        let mut raw = std::ptr::null_mut();
        let found = unsafe { DllGetClassObject(&CLSID_MENU, &IClassFactory::IID, &mut raw) };
        assert!(found.is_ok());
        let factory = unsafe { IClassFactory::from_raw(raw) };

        let init: IShellExtInit = unsafe { factory.CreateInstance(None::<&IUnknown>) }.unwrap();
        let menu: IContextMenu = init.cast().unwrap();

        assert_eq!(DllCanUnloadNow(), S_FALSE, "objects are alive");
        drop((menu, init, factory));
        assert_eq!(DllCanUnloadNow(), S_OK, "everything was released");
    }

    #[test]
    fn an_unknown_class_is_refused() {
        let _scenario = Scenario::new('Y', true);
        let mut raw = std::ptr::null_mut();

        let found =
            unsafe { DllGetClassObject(&GUID::from_u128(0x1234), &IClassFactory::IID, &mut raw) };

        assert_eq!(found, CLASS_E_CLASSNOTAVAILABLE);
        assert!(raw.is_null());
    }

    #[test]
    fn a_drop_on_a_drive_adds_copy_and_move_and_reports_how_many() {
        let _scenario = Scenario::new('Y', true);
        let (_object, menu) = handler_for(drop_onto_drive());
        let popup = unsafe { CreatePopupMenu() }.unwrap();

        let added = unsafe { menu.QueryContextMenu(popup, 0, 100, 200, 0) };

        assert_eq!(added.0, 2);
        assert_eq!(
            menu_texts(popup),
            vec![
                "Copy here with Bifrost Drive",
                "Move here with Bifrost Drive"
            ]
        );
        unsafe { DestroyMenu(popup) }.unwrap();
    }

    #[test]
    fn nothing_is_added_when_the_setting_is_off_or_only_the_default_is_wanted() {
        let popup = unsafe { CreatePopupMenu() }.unwrap();
        {
            let _scenario = Scenario::new('Y', false);
            let (_object, menu) = handler_for(drop_onto_drive());
            assert_eq!(unsafe { menu.QueryContextMenu(popup, 0, 100, 200, 0) }.0, 0);
        }
        {
            let _scenario = Scenario::new('Y', true);
            let (_object, menu) = handler_for(drop_onto_drive());
            let only_default =
                unsafe { menu.QueryContextMenu(popup, 0, 100, 200, CMF_DEFAULTONLY) };
            assert_eq!(only_default.0, 0);
        }
        assert!(menu_texts(popup).is_empty());
        unsafe { DestroyMenu(popup) }.unwrap();
    }

    #[test]
    fn the_shell_can_ask_for_each_commands_verb_and_help_text() {
        let _scenario = Scenario::new('Y', true);
        let (_object, menu) = handler_for(drop_onto_drive());
        let popup = unsafe { CreatePopupMenu() }.unwrap();
        let _ = unsafe { menu.QueryContextMenu(popup, 0, 100, 200, 0) };

        let ask = |index: usize, kind: u32| {
            let mut buffer = [0u16; 256];
            unsafe {
                menu.GetCommandString(
                    index,
                    kind,
                    None,
                    PSTR(buffer.as_mut_ptr().cast()),
                    buffer.len() as u32,
                )
            }
            .unwrap();
            let length = buffer.iter().position(|&unit| unit == 0).unwrap();
            String::from_utf16_lossy(&buffer[..length])
        };

        assert_eq!(ask(0, GCS_VERBW), "bifrost.copy");
        assert_eq!(ask(1, GCS_VERBW), "bifrost.move");
        assert!(ask(1, GCS_HELPTEXTW).starts_with("Move"));
        let mut spare = [0u8; 64];
        let unknown =
            unsafe { menu.GetCommandString(5, GCS_VERBW, None, PSTR(spare.as_mut_ptr()), 32) };
        assert!(unknown.is_err());
        unsafe { DestroyMenu(popup) }.unwrap();
    }

    #[test]
    fn a_command_is_found_by_position_or_by_verb_name() {
        let _scenario = Scenario::new('Y', true);
        let (object, menu) = handler_for(drop_onto_drive());
        let popup = unsafe { CreatePopupMenu() }.unwrap();
        let _ = unsafe { menu.QueryContextMenu(popup, 0, 100, 200, 0) };
        let mut info: CMINVOKECOMMANDINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<CMINVOKECOMMANDINFO>() as u32;

        info.lpVerb = PCSTR(std::ptr::without_provenance(1));
        assert_eq!(object.entry_for(&info).unwrap().mode, Mode::Move);

        info.lpVerb = PCSTR(c"bifrost.copy".as_ptr().cast());
        assert_eq!(object.entry_for(&info).unwrap().mode, Mode::Copy);

        info.lpVerb = PCSTR(c"someone.else".as_ptr().cast());
        assert_eq!(object.entry_for(&info), None);
        unsafe { DestroyMenu(popup) }.unwrap();
    }

    #[test]
    fn choosing_an_item_saves_the_request_and_starts_the_app_with_it() {
        let data = tempfile::tempdir().unwrap();
        let launched = RefCell::new(None);
        let gesture = drop_onto_drive();
        let entry = decision::entries(&gesture, true, &|_| true)[1];

        perform(
            &gesture,
            &entry,
            data.path(),
            Path::new(r"C:\App\bifrost-drive.exe"),
            &|app, file| {
                *launched.borrow_mut() = Some((app.to_path_buf(), file.to_path_buf()));
                Ok(())
            },
        )
        .unwrap();

        let (app, file) = launched.into_inner().unwrap();
        assert_eq!(app, Path::new(r"C:\App\bifrost-drive.exe"));
        assert_eq!(
            file.parent().unwrap(),
            data.path()
                .join("com.bifrost.drive")
                .join("transfer-requests")
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["mode"], "move");
        assert_eq!(saved["destination"], r"Y:\Filmer");
    }

    #[test]
    fn a_request_the_app_never_received_is_not_left_behind() {
        let data = tempfile::tempdir().unwrap();
        let gesture = drop_onto_drive();
        let entry = decision::entries(&gesture, true, &|_| true)[0];

        let outcome = perform(
            &gesture,
            &entry,
            data.path(),
            Path::new("missing.exe"),
            &|_, _| Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
        );

        assert!(outcome.unwrap_err().contains("could not be started"));
        let requests = data
            .path()
            .join("com.bifrost.drive")
            .join("transfer-requests");
        assert_eq!(std::fs::read_dir(requests).unwrap().count(), 0);
    }

    #[test]
    fn explorer_hands_over_a_drop_a_paste_or_a_selection_and_each_is_told_apart() {
        let clipboard = || Some((vec![PathBuf::from(r"C:\a.mkv")], Mode::Move));
        let folder = || Some(PathBuf::from(r"Y:\Filmer"));
        let files = || Some(vec![PathBuf::from(r"C:\a.mkv")]);

        assert!(matches!(
            gesture_for(folder(), files(), || None, |_| false).as_slice(),
            [Gesture::Drop { .. }]
        ));
        assert!(matches!(
            gesture_for(folder(), None, clipboard, |_| false).as_slice(),
            [Gesture::Paste {
                clipboard_mode: Mode::Move,
                ..
            }]
        ));
        assert!(gesture_for(folder(), None, || None, |_| false).is_empty());
        let selected = Some(vec![PathBuf::from(r"Y:\Filmer")]);
        assert!(matches!(
            gesture_for(None, selected.clone(), clipboard, |_| true).as_slice(),
            [Gesture::Selection { .. }, Gesture::Paste { .. }]
        ));
        assert!(matches!(
            gesture_for(None, selected, clipboard, |_| false).as_slice(),
            [Gesture::Selection { .. }]
        ));
        assert!(gesture_for(None, None, clipboard, |_| true).is_empty());
    }

    #[test]
    fn right_clicking_a_folder_is_a_paste_target_even_when_explorer_also_names_its_parent() {
        let clipboard = || Some((vec![PathBuf::from(r"C:\a.mkv")], Mode::Copy));
        let parent = Some(PathBuf::from(r"Y:\"));
        let clicked = Some(vec![PathBuf::from(r"Y:\Filmer")]);

        let readings = gesture_for(parent.clone(), clicked.clone(), clipboard, |_| true);

        assert_eq!(
            readings,
            vec![
                Gesture::Selection {
                    items: vec![PathBuf::from(r"Y:\Filmer")],
                },
                Gesture::Paste {
                    sources: vec![PathBuf::from(r"C:\a.mkv")],
                    target: PathBuf::from(r"Y:\Filmer"),
                    clipboard_mode: Mode::Copy,
                },
            ]
        );
        // A file was clicked: it can be deleted, but there is nothing to paste into.
        assert!(matches!(
            gesture_for(parent, clicked, clipboard, |_| false).as_slice(),
            [Gesture::Selection { .. }]
        ));
    }

    #[test]
    fn a_selection_on_a_bifrost_drive_offers_delete_next_to_paste() {
        let _scenario = Scenario::new('Y', true);
        let readings = gesture_for(
            None,
            Some(vec![PathBuf::from(r"Y:\Filmer")]),
            || Some((vec![PathBuf::from(r"C:\a.mkv")], Mode::Copy)),
            |_| true,
        );

        let offered =
            decision::entries_for(&readings, true, &|path| volume::is_bifrost_drive(path));

        assert_eq!(
            offered.iter().map(|entry| entry.label).collect::<Vec<_>>(),
            vec!["Delete with Bifrost Drive", "Paste with Bifrost Drive"]
        );
        assert_eq!(
            decision::gesture_for_entry(&readings, &offered[0]),
            Some(&readings[0])
        );
        assert_eq!(
            decision::gesture_for_entry(&readings, &offered[1]),
            Some(&readings[1])
        );
    }

    #[test]
    fn choosing_delete_saves_a_request_without_a_destination() {
        let data = tempfile::tempdir().unwrap();
        let launched = RefCell::new(false);
        let gesture = Gesture::Selection {
            items: vec![PathBuf::from(r"Y:\Filmer\a.mkv")],
        };
        let entry = decision::entries(&gesture, true, &|_| true)[0];

        perform(
            &gesture,
            &entry,
            data.path(),
            Path::new(r"C:\App\bifrost-drive.exe"),
            &|_, file| {
                let saved: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
                assert_eq!(saved["mode"], "delete");
                assert_eq!(saved["sources"][0], r"Y:\Filmer\a.mkv");
                assert_eq!(saved["destination"], "");
                *launched.borrow_mut() = true;
                Ok(())
            },
        )
        .unwrap();

        assert!(launched.into_inner());
    }

    fn shell_item(path: &Path) -> *mut ITEMIDLIST {
        let text = wide(&path.to_string_lossy());
        let mut pidl = std::ptr::null_mut();
        unsafe { SHParseDisplayName(PCWSTR(text.as_ptr()), None, &mut pidl, 0, None) }.unwrap();
        pidl
    }

    /// A data object like the one Explorer passes for a selection.
    fn explorer_selection(file: &Path) -> IDataObject {
        let pidl = shell_item(file);
        let mut child = std::ptr::null_mut();
        let folder: IShellFolder = unsafe { SHBindToParent(pidl, Some(&mut child)) }.unwrap();
        let data =
            unsafe { folder.GetUIObjectOf(HWND::default(), &[child.cast_const()], None) }.unwrap();
        unsafe { CoTaskMemFree(Some(pidl.cast())) };
        data
    }

    #[test]
    fn a_real_explorer_selection_dropped_on_a_folder_produces_the_menu() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("clip.mkv");
        std::fs::write(&file, b"x").unwrap();
        let directory = tempfile::tempdir_in(root.path()).unwrap();
        let letter = volume::drive_letter(directory.path()).expect("the temp folder is on a drive");
        let _scenario = Scenario::new(letter, true);
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let selection = explorer_selection(&file);
        let target = shell_item(directory.path());

        let object = ComObject::new(MenuHandler::new());
        let init = object.to_interface::<IShellExtInit>();
        unsafe { init.Initialize(Some(target.cast_const()), &selection, None) }.unwrap();
        unsafe { CoTaskMemFree(Some(target.cast())) };

        assert_eq!(
            object.gestures.borrow().as_slice(),
            [Gesture::Drop {
                sources: vec![file.clone()],
                target: directory.path().to_path_buf(),
            }]
        );
        let popup = unsafe { CreatePopupMenu() }.unwrap();
        let menu = object.to_interface::<IContextMenu>();
        assert_eq!(unsafe { menu.QueryContextMenu(popup, 0, 1, 100, 0) }.0, 2);
        unsafe { DestroyMenu(popup) }.unwrap();
    }
}
