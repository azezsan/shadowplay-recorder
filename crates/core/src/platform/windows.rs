use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HWND, LPARAM};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, GetExitCodeProcess, INFINITE, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindowTextLengthW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_HIDE, SW_RESTORE, SetForegroundWindow,
    ShowWindow,
};
use windows::core::{BOOL, PCWSTR, w};

use super::{DiskSpace, ElevationError};

fn wide(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

/// Ids of every process with this executable name.
fn process_ids(name: &str) -> Vec<u32> {
    let mut ids = Vec::new();
    // SAFETY: the snapshot handle is valid until closed, and `entry` has its
    // size set as the API requires.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return ids;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let exe = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if exe.eq_ignore_ascii_case(name) {
                ids.push(entry.th32ProcessID);
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    ids
}

pub fn is_process_running(name: &str) -> bool {
    !process_ids(name).is_empty()
}

pub fn steam_root() -> Option<PathBuf> {
    let mut buffer = [0u16; 1024];
    let mut bytes = size_of_val(&buffer) as u32;
    // SAFETY: `buffer` is writable for `bytes` bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Valve\\Steam"),
            w!("SteamPath"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            Some(&mut bytes),
        )
    };
    if status.is_err() {
        return None;
    }
    let len = buffer.iter().position(|&c| c == 0)?;
    let path = PathBuf::from(String::from_utf16_lossy(&buffer[..len]));
    path.is_dir().then_some(path)
}

pub fn disk_space(path: &Path) -> Option<DiskSpace> {
    let path = wide(path);
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: `path` is NUL-terminated and outlives the call, and both
    // outputs are writable u64s.
    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(path.as_ptr()),
            Some(&mut free),
            Some(&mut total),
            None,
        )
    }
    .ok()?;
    Some(DiskSpace { free, total })
}

pub fn run_elevated(program: &Path, arguments: &str) -> Result<u32, ElevationError> {
    let file = wide(program);
    let parameters = wide(arguments);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` and the strings it points to outlive the call, and the
    // process handle it returns is closed below.
    unsafe {
        ShellExecuteExW(&mut info).map_err(|e| {
            if e.code() == ERROR_CANCELLED.to_hresult() {
                ElevationError::Declined
            } else {
                ElevationError::Failed(e.message())
            }
        })?;
        if info.hProcess.is_invalid() {
            return Err(ElevationError::Failed(
                "the elevated process did not start".into(),
            ));
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let result = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        result
            .map(|()| code)
            .map_err(|e| ElevationError::Failed(e.message()))
    }
}

struct WindowSearch {
    process_ids: Vec<u32>,
    found: Option<HWND>,
}

/// Stops at the first visible, titled top-level window owned by one of the
/// wanted processes. Source 2 also creates untitled helper windows.
unsafe extern "system" fn visit_window(window: HWND, param: LPARAM) -> BOOL {
    // SAFETY: `param` is the `WindowSearch` passed to `EnumWindows` below,
    // which outlives the enumeration.
    let search = unsafe { &mut *(param.0 as *mut WindowSearch) };
    let mut owner = 0u32;
    // SAFETY: `window` comes from the enumeration and `owner` is writable.
    let wanted = unsafe {
        IsWindowVisible(window).as_bool()
            && GetWindowThreadProcessId(window, Some(&mut owner)) != 0
            && search.process_ids.contains(&owner)
            && GetWindowTextLengthW(window) > 0
    };
    if wanted {
        search.found = Some(window);
    }
    BOOL::from(!wanted)
}

fn find_window(name: &str) -> Option<HWND> {
    let mut search = WindowSearch {
        process_ids: process_ids(name),
        found: None,
    };
    if search.process_ids.is_empty() {
        return None;
    }
    // SAFETY: the callback only uses `search` during this call. The result is
    // an error when the callback stops the enumeration early, which is the
    // success case here.
    let _ = unsafe { EnumWindows(Some(visit_window), LPARAM(&raw mut search as isize)) };
    search.found
}

pub fn is_foreground(name: &str) -> bool {
    let mut owner = 0u32;
    // SAFETY: plain queries; a null foreground window yields thread id 0.
    let has_owner = unsafe {
        let foreground = GetForegroundWindow();
        !foreground.is_invalid() && GetWindowThreadProcessId(foreground, Some(&mut owner)) != 0
    };
    has_owner && process_ids(name).contains(&owner)
}

pub fn focus_process(name: &str) -> bool {
    let Some(window) = find_window(name) else {
        return false;
    };
    // Windows only grants the foreground to the thread that owns the current
    // foreground window, or one with recent input. Attaching this thread's
    // input queue to that thread makes the request count as coming from it.
    // SAFETY: all handles come from the system during this call, and the
    // input queues are detached again before returning.
    unsafe {
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        let foreground = GetForegroundWindow();
        let this_thread = GetCurrentThreadId();
        let foreground_thread = GetWindowThreadProcessId(foreground, None);
        let attached = foreground_thread != 0
            && foreground_thread != this_thread
            && AttachThreadInput(this_thread, foreground_thread, true).as_bool();
        let _ = BringWindowToTop(window);
        let _ = SetForegroundWindow(window);
        if attached {
            let _ = AttachThreadInput(this_thread, foreground_thread, false);
        }
        GetForegroundWindow() == window
    }
}
