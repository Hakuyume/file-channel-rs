use slab::Slab;
use std::task::Waker;

#[derive(Default)]
pub(crate) struct State {
    pub(crate) offset: u64,
    pub(crate) is_closed: bool,
    pub(crate) wakers: Slab<Option<Waker>>,
}
