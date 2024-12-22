mod buf;
mod reader;
mod runtime;
mod writer;

// https://github.com/rust-lang/rust/blob/426d1734238e3c5f52e935ba4f617f3a9f43b59d/library/std/src/sys_common/io.rs#L3
pub(crate) const DEFAULT_BUF_SIZE: usize = 8 * 1024;

pub(crate) use reader::Reader;
pub(crate) use writer::Writer;
