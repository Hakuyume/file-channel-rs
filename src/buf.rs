use std::cmp;
use std::mem::MaybeUninit;

#[derive(Clone, Default)]
pub(crate) struct Buf {
    data: Box<[MaybeUninit<u8>]>,
    offset: usize,
    len: usize,
}

impl Buf {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Box::new_uninit_slice(capacity),
            offset: 0,
            len: 0,
        }
    }

    pub(crate) fn filled(&self) -> (&[u8], &[u8]) {
        let Self { data, offset, len } = self;
        let capacity = data.len();
        if *offset + *len < capacity {
            // d0(unfilled): ..offset
            // d1(filled): offset..(offset + len)
            // d2(unfilled): (offset + len)..
            let (data, _) = data.split_at(*offset + *len);
            let (_, d1) = data.split_at(*offset);
            unsafe { (crate::unstable::slice_assume_init_ref(d1), &[]) }
        } else {
            // d0(filled): ..(offset + len - capacity)
            // d1(unfilled): (offset + len - capacity)..offset
            // d2(filled): offset..
            let (data, d2) = data.split_at(*offset);
            let (d0, _) = data.split_at(*offset + *len - capacity);
            unsafe {
                (
                    crate::unstable::slice_assume_init_ref(d2),
                    crate::unstable::slice_assume_init_ref(d0),
                )
            }
        }
    }

    pub(crate) fn unfilled_mut(&mut self) -> (&mut [MaybeUninit<u8>], &mut [MaybeUninit<u8>]) {
        let Self { data, offset, len } = self;
        let capacity = data.len();
        if *offset + *len < capacity {
            // d0(unfilled): ..offset
            // d1(filled): offset..(offset + len)
            // d2(unfilled): (offset + len)..
            let (data, d2) = data.split_at_mut(*offset + *len);
            let (d0, _) = data.split_at_mut(*offset);
            (d2, d0)
        } else {
            // d0(filled): ..(offset + len - capacity)
            // d1(unfilled): (offset + len - capacity)..offset
            // d2(filled): offset..
            let (data, _) = data.split_at_mut(*offset);
            let (_, d1) = data.split_at_mut(*offset + *len - capacity);
            (d1, &mut [])
        }
    }

    pub(crate) fn consume(&mut self, amt: usize) {
        assert!(amt <= self.len);
        self.offset += amt;
        self.len -= amt;
        if self.offset >= self.data.len() {
            self.offset -= self.data.len();
        }
    }

    pub(crate) unsafe fn advance(&mut self, n: usize) {
        assert!(n <= self.data.len() - self.len);
        self.len += n;
    }

    pub(crate) fn capacity(&self) -> usize {
        self.data.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn extend_from_slice(&mut self, other: &[u8]) {
        assert!(self.len() + other.len() <= self.capacity());
        unsafe {
            let (data, _) = self.unfilled_mut();
            let n = cmp::min(data.len(), other.len());
            crate::unstable::slice_assume_init_mut(&mut data[..n]).copy_from_slice(&other[..n]);
            self.advance(n);
            let other = &other[n..];

            let (data, _) = self.unfilled_mut();
            let n = cmp::min(data.len(), other.len());
            crate::unstable::slice_assume_init_mut(&mut data[..n]).copy_from_slice(&other[..n]);
            self.advance(n);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test() {
        let mut buf = super::Buf::with_capacity(8);
        let (head, tail) = buf.filled();
        assert!(head.is_empty());
        assert!(tail.is_empty());
        let (head, tail) = buf.unfilled_mut();
        assert_eq!(head.len(), 8);
        assert!(tail.is_empty());
        assert_eq!(buf.capacity(), 8);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);

        buf.extend_from_slice(b"hello");
        let (head, tail) = buf.filled();
        assert_eq!(head, b"hello");
        assert!(tail.is_empty());
        let (head, tail) = buf.unfilled_mut();
        assert_eq!(head.len(), 3);
        assert!(tail.is_empty());
        assert_eq!(buf.capacity(), 8);
        assert!(!buf.is_empty());
        assert_eq!(buf.len(), 5);

        buf.consume(4);
        let (head, tail) = buf.filled();
        assert_eq!(head, b"o");
        assert!(tail.is_empty());
        let (head, tail) = buf.unfilled_mut();
        assert_eq!(head.len(), 3);
        assert_eq!(tail.len(), 4);
        assert_eq!(buf.capacity(), 8);
        assert!(!buf.is_empty());
        assert_eq!(buf.len(), 1);

        buf.extend_from_slice(b" world");
        let (head, tail) = buf.filled();
        assert_eq!(head, b"o wo");
        assert_eq!(tail, b"rld");
        let (head, tail) = buf.unfilled_mut();
        assert_eq!(head.len(), 1);
        assert!(tail.is_empty());
        assert_eq!(buf.capacity(), 8);
        assert!(!buf.is_empty());
        assert_eq!(buf.len(), 7);
    }
}
