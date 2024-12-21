use std::mem::MaybeUninit;
use std::ops;

#[derive(Clone, Default)]
pub(super) struct Buf {
    data: Vec<u8>,
    offset: usize,
}

impl ops::Deref for Buf {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data[self.offset..]
    }
}

impl Buf {
    pub(super) fn advance(&mut self, n: usize) {
        self.offset += n;
    }

    pub(super) fn clear(&mut self) {
        self.data.clear();
        self.offset = 0;
    }

    pub(super) fn extend_from_slice(&mut self, other: &[u8]) {
        self.reclaim();
        self.data.extend_from_slice(other);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.data.len() == self.offset
    }

    pub(super) fn len(&self) -> usize {
        self.data.len() - self.offset
    }

    pub(super) fn reserve(&mut self, additional: usize) {
        self.reclaim();
        self.data.reserve(additional);
    }

    pub(super) unsafe fn set_len(&mut self, new_len: usize) {
        self.data.set_len(self.offset + new_len);
    }

    pub(super) fn spare_capacity_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        self.data.spare_capacity_mut()
    }

    fn reclaim(&mut self) {
        if self.is_empty() {
            self.clear()
        }
    }
}
