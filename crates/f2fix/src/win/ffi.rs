//! Minimal Win32 declarations (no windows-sys dependency).

#![allow(non_snake_case, clippy::upper_case_acronyms)]

use core::ffi::c_void;

pub type HMODULE = *mut c_void;
pub type HANDLE = *mut c_void;
pub type BOOL = i32;

pub const DLL_PROCESS_ATTACH: u32 = 1;
pub const DLL_PROCESS_DETACH: u32 = 0;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;

pub type ThreadStart = unsafe extern "system" fn(*mut c_void) -> u32;

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleA(name: *const u8) -> HMODULE;
    pub fn GetModuleFileNameW(m: HMODULE, buf: *mut u16, len: u32) -> u32;
    pub fn GetProcAddress(m: HMODULE, name: *const u8) -> *mut c_void;
    pub fn LoadLibraryW(name: *const u16) -> HMODULE;
    pub fn LoadLibraryA(name: *const u8) -> HMODULE;
    pub fn GetSystemDirectoryW(buf: *mut u16, len: u32) -> u32;
    pub fn DisableThreadLibraryCalls(m: HMODULE) -> BOOL;
    pub fn CreateThread(
        sa: *mut c_void,
        stack: usize,
        start: ThreadStart,
        param: *mut c_void,
        flags: u32,
        tid: *mut u32,
    ) -> HANDLE;
    pub fn CloseHandle(h: HANDLE) -> BOOL;
    pub fn VirtualProtect(addr: *mut c_void, size: usize, new: u32, old: *mut u32) -> BOOL;
    pub fn GetCurrentProcess() -> HANDLE;
    pub fn GetCurrentProcessId() -> u32;
    pub fn GetCurrentThreadId() -> u32;
    pub fn TerminateProcess(h: HANDLE, code: u32) -> BOOL;
    pub fn GetLastError() -> u32;
    pub fn SetLastError(e: u32);
    pub fn GetTickCount() -> u32;
    pub fn FlushInstructionCache(h: HANDLE, addr: *const c_void, size: usize) -> BOOL;
}

#[link(name = "user32")]
extern "system" {
    pub fn GetSystemMetrics(i: i32) -> i32;
}

pub const SM_XVIRTUALSCREEN: i32 = 76;
pub const SM_YVIRTUALSCREEN: i32 = 77;
