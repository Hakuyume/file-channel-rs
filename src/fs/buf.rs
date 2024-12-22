use std::{cmp, mem::MaybeUninit};

#[derive(Clone)]
pub(super) struct Buf {
    data: Box<[MaybeUninit<u8>]>,
    offset: usize,
    len: usize,
}

impl Buf {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Box::new_uninit_slice(capacity),
            offset: 0,
            len: 0,
        }
    }

    pub(super) fn as_slices(&self) -> (&[u8], &[u8]) {
        let Self { data, offset, len } = self;
        let capacity = data.len();
        if *offset + *len < capacity {
            // d0(uninit): ..offset
            // d1(init): offset..(offset + len)
            // d2(uninit): (offset + len)..
            let (data, _) = data.split_at(*offset + *len);
            let (_, d1) = data.split_at(*offset);
            unsafe { (slice_assume_init_ref(d1), &[]) }
        } else {
            // d0(init): ..(offset + len - capacity)
            // d1(uninit): (offset + len - capacity)..offset
            // d2(init): offset..
            let (data, d2) = data.split_at(*offset);
            let (d0, _) = data.split_at(*offset + *len - capacity);
            unsafe { (slice_assume_init_ref(d2), slice_assume_init_ref(d0)) }
        }
    }

    pub(super) fn spare_capacity_mut(
        &mut self,
    ) -> (&mut [MaybeUninit<u8>], &mut [MaybeUninit<u8>]) {
        let Self { data, offset, len } = self;
        let capacity = data.len();
        if *offset + *len < capacity {
            // d0(uninit): ..offset
            // d1(init): offset..(offset + len)
            // d2(uninit): (offset + len)..
            let (data, d2) = data.split_at_mut(*offset + *len);
            let (d0, _) = data.split_at_mut(*offset);
            (d2, d0)
        } else {
            // d0(init): ..(offset + len - capacity)
            // d1(uninit): (offset + len - capacity)..offset
            // d2(init): offset..
            let (data, _) = data.split_at_mut(*offset);
            let (_, d1) = data.split_at_mut(*offset + *len - capacity);
            (d1, &mut [])
        }
    }

    pub(super) fn consume(&mut self, amt: usize) {
        assert!(amt <= self.len);
        self.offset += amt;
        self.len -= amt;
        if self.offset >= self.data.len() {
            self.offset -= self.data.len();
        }
    }

    pub(super) unsafe fn set_init(&mut self, n: usize) {
        assert!(n <= self.data.len() - self.len);
        self.len += n;
    }

    pub(super) fn capacity(&self) -> usize {
        self.data.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn extend_from_slice(&mut self, other: &[u8]) {
        assert!(self.len() + other.len() <= self.capacity());
        unsafe {
            let (data, _) = self.spare_capacity_mut();
            let n = cmp::min(data.len(), other.len());
            slice_assume_init_mut(&mut data[..n]).copy_from_slice(&other[..n]);
            self.set_init(n);
            let other = &other[n..];

            let (data, _) = self.spare_capacity_mut();
            let n = cmp::min(data.len(), other.len());
            slice_assume_init_mut(&mut data[..n]).copy_from_slice(&other[..n]);
            self.set_init(n);
        }
    }
}

// https://github.com/rust-lang/rust/issues/63569
pub(crate) const unsafe fn slice_assume_init_ref<T>(slice: &[MaybeUninit<T>]) -> &[T] {
    unsafe { &*(slice as *const [_] as *const [T]) }
}
pub(crate) const unsafe fn slice_assume_init_mut<T>(slice: &mut [MaybeUninit<T>]) -> &mut [T] {
    unsafe { &mut *(slice as *mut [_] as *mut [T]) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test() {
        let mut buf = super::Buf::with_capacity(8);
        let (head, tail) = buf.as_slices();
        assert!(head.is_empty());
        assert!(tail.is_empty());
        let (head, tail) = buf.spare_capacity_mut();
        assert_eq!(head.len(), 8);
        assert!(tail.is_empty());
        assert_eq!(buf.capacity(), 8);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);

        buf.extend_from_slice(b"hello");
        let (head, tail) = buf.as_slices();
        assert_eq!(head, b"hello");
        assert!(tail.is_empty());
        let (head, tail) = buf.spare_capacity_mut();
        assert_eq!(head.len(), 3);
        assert!(tail.is_empty());
        assert_eq!(buf.capacity(), 8);
        assert!(!buf.is_empty());
        assert_eq!(buf.len(), 5);

        buf.consume(4);
        let (head, tail) = buf.as_slices();
        assert_eq!(head, b"o");
        assert!(tail.is_empty());
        let (head, tail) = buf.spare_capacity_mut();
        assert_eq!(head.len(), 3);
        assert_eq!(tail.len(), 4);
        assert_eq!(buf.capacity(), 8);
        assert!(!buf.is_empty());
        assert_eq!(buf.len(), 1);

        buf.extend_from_slice(b" world");
        let (head, tail) = buf.as_slices();
        assert_eq!(head, b"o wo");
        assert_eq!(tail, b"rld");
        let (head, tail) = buf.spare_capacity_mut();
        assert_eq!(head.len(), 1);
        assert!(tail.is_empty());
        assert_eq!(buf.capacity(), 8);
        assert!(!buf.is_empty());
        assert_eq!(buf.len(), 7);
    }
}
