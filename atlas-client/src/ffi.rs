use crate::AtlasClient;
use std::ffi::CStr;
use std::os::raw::c_char;

#[unsafe(no_mangle)]
pub extern "C" fn atlas_client_create(instance_id: u32) -> *mut AtlasClient {
    match AtlasClient::new(instance_id) {
        Ok(client) => Box::into_raw(Box::new(client)),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn atlas_client_destroy(client: *mut AtlasClient) {
    if !client.is_null() {
        drop(unsafe { Box::from_raw(client) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn atlas_client_open(
    client: *mut AtlasClient,
    path: *const c_char,
    flags: u32,
) -> i64 {
    let client = unsafe { &mut *client };
    let path = unsafe { CStr::from_ptr(path) }.to_str().unwrap_or("");
    match client.open(path, flags) {
        Ok(fd) => fd as i64,
        Err(e) => e as i64,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn atlas_client_read(
    client: *mut AtlasClient,
    fd: u64,
    buf: *mut u8,
    offset: u64,
    len: u32,
) -> i32 {
    let client = unsafe { &mut *client };
    let buf = unsafe { std::slice::from_raw_parts_mut(buf, len as usize) };
    match client.read(fd, buf, offset) {
        Ok(n) => n as i32,
        Err(e) => e,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn atlas_client_write(
    client: *mut AtlasClient,
    fd: u64,
    buf: *const u8,
    offset: u64,
    len: u32,
) -> i32 {
    let client = unsafe { &mut *client };
    let buf = unsafe { std::slice::from_raw_parts(buf, len as usize) };
    match client.write(fd, buf, offset) {
        Ok(n) => n as i32,
        Err(e) => e,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn atlas_client_sync(client: *mut AtlasClient, fd: u64) -> i32 {
    let client = unsafe { &mut *client };
    match client.sync(fd) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn atlas_client_close(client: *mut AtlasClient, fd: u64) -> i32 {
    let client = unsafe { &mut *client };
    match client.close(fd) {
        Ok(()) => 0,
        Err(e) => e,
    }
}
