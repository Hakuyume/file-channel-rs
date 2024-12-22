use std::fs::File;
use std::io::{self, IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;

// https://github.com/rust-lang/rust/issues/63569
pub(crate) const unsafe fn slice_assume_init_ref<T>(slice: &[MaybeUninit<T>]) -> &[T] {
    unsafe { &*(slice as *const [_] as *const [T]) }
}
pub(crate) const unsafe fn slice_assume_init_mut<T>(slice: &mut [MaybeUninit<T>]) -> &mut [T] {
    unsafe { &mut *(slice as *mut [_] as *mut [T]) }
}

// https://github.com/rust-lang/rust/issues/89517
pub(crate) fn read_vectored_at(
    file: &File,
    bufs: &mut [IoSliceMut<'_>],
    offset: u64,
) -> io::Result<usize> {
    unsafe {
        let n = libc::preadv(
            file.as_raw_fd(),
            bufs.as_mut_ptr().cast(),
            bufs.len() as _,
            offset as _,
        );
        if n >= 0 {
            Ok(n as _)
        } else {
            Err(io::Error::last_os_error())
        }
    }
}
pub(crate) fn write_vectored_at(
    file: &File,
    bufs: &[IoSlice<'_>],
    offset: u64,
) -> io::Result<usize> {
    unsafe {
        let n = libc::pwritev(
            file.as_raw_fd(),
            bufs.as_ptr().cast(),
            bufs.len() as _,
            offset as _,
        );
        if n >= 0 {
            Ok(n as _)
        } else {
            Err(io::Error::last_os_error())
        }
    }
}
