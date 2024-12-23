use std::mem::MaybeUninit;

#[derive(Clone)]
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

    pub(crate) fn chunks(&self) -> [&[u8]; 2] {
        let Self { data, offset, len } = self;
        let capacity = data.len();
        if *offset + *len < capacity {
            // d0(uninit): ..offset
            // d1(init): offset..(offset + len)
            // d2(uninit): (offset + len)..
            let (data, _) = data.split_at(*offset + *len);
            let (_, d1) = data.split_at(*offset);
            unsafe { [crate::unstable::slice_assume_init_ref(d1), &[]] }
        } else {
            // d0(init): ..(offset + len - capacity)
            // d1(uninit): (offset + len - capacity)..offset
            // d2(init): offset..
            let (data, d2) = data.split_at(*offset);
            let (d0, _) = data.split_at(*offset + *len - capacity);
            unsafe {
                [
                    crate::unstable::slice_assume_init_ref(d2),
                    crate::unstable::slice_assume_init_ref(d0),
                ]
            }
        }
    }

    pub(crate) fn chunks_mut(&mut self) -> [&mut [MaybeUninit<u8>]; 2] {
        let Self { data, offset, len } = self;
        let capacity = data.len();
        if *offset + *len < capacity {
            // d0(uninit): ..offset
            // d1(init): offset..(offset + len)
            // d2(uninit): (offset + len)..
            let (data, d2) = data.split_at_mut(*offset + *len);
            let (d0, _) = data.split_at_mut(*offset);
            [d2, d0]
        } else {
            // d0(init): ..(offset + len - capacity)
            // d1(uninit): (offset + len - capacity)..offset
            // d2(init): offset..
            let (data, _) = data.split_at_mut(*offset);
            let (_, d1) = data.split_at_mut(*offset + *len - capacity);
            [d1, &mut []]
        }
    }
}

impl bytes::Buf for Buf {
    fn advance(&mut self, cnt: usize) {
        assert!(cnt <= self.remaining());
        self.offset += cnt;
        self.len -= cnt;
        if self.offset >= self.data.len() {
            self.offset -= self.data.len();
        }
    }

    fn chunk(&self) -> &[u8] {
        let [chunk, _] = self.chunks();
        chunk
    }

    fn remaining(&self) -> usize {
        self.len
    }
}

unsafe impl bytes::BufMut for Buf {
    unsafe fn advance_mut(&mut self, cnt: usize) {
        assert!(cnt <= self.remaining_mut());
        self.len += cnt;
    }

    fn chunk_mut(&mut self) -> &mut bytes::buf::UninitSlice {
        let [chunk, _] = self.chunks_mut();
        bytes::buf::UninitSlice::uninit(chunk)
    }

    fn remaining_mut(&self) -> usize {
        self.data.len() - self.len
    }
}

#[cfg(test)]
mod tests {
    use bytes::{Buf, BufMut};

    #[test]
    fn test() {
        let mut buf = super::Buf::with_capacity(8);
        let [chunk0, chunk1] = buf.chunks();
        assert!(chunk0.is_empty());
        assert!(chunk1.is_empty());
        let [chunk0, chunk1] = buf.chunks_mut();
        assert_eq!(chunk0.len(), 8);
        assert!(chunk1.is_empty());
        assert!(buf.chunk().is_empty());
        assert_eq!(buf.remaining(), 0);
        assert_eq!(buf.chunk_mut().len(), 8);
        assert_eq!(buf.remaining_mut(), 8);

        buf.put_slice(b"hello");
        let [chunk0, chunk1] = buf.chunks();
        assert_eq!(chunk0, b"hello");
        assert!(chunk1.is_empty());
        let [chunk0, chunk1] = buf.chunks_mut();
        assert_eq!(chunk0.len(), 3);
        assert!(chunk1.is_empty());
        assert_eq!(buf.chunk(), b"hello");
        assert_eq!(buf.remaining(), 5);
        assert_eq!(buf.chunk_mut().len(), 3);
        assert_eq!(buf.remaining_mut(), 3);

        buf.advance(4);
        let [chunk0, chunk1] = buf.chunks();
        assert_eq!(chunk0, b"o");
        assert!(chunk1.is_empty());
        let [chunk0, chunk1] = buf.chunks_mut();
        assert_eq!(chunk0.len(), 3);
        assert_eq!(chunk1.len(), 4);
        assert_eq!(buf.chunk(), b"o");
        assert_eq!(buf.remaining(), 1);
        assert_eq!(buf.chunk_mut().len(), 3);
        assert_eq!(buf.remaining_mut(), 7);

        buf.put_slice(b" world");
        let [chunk0, chunk1] = buf.chunks();
        assert_eq!(chunk0, b"o wo");
        assert_eq!(chunk1, b"rld");
        let [chunk0, chunk1] = buf.chunks_mut();
        assert_eq!(chunk0.len(), 1);
        assert!(chunk1.is_empty());
        assert_eq!(buf.chunk(), b"o wo");
        assert_eq!(buf.remaining(), 7);
        assert_eq!(buf.chunk_mut().len(), 1);
        assert_eq!(buf.remaining_mut(), 1);
    }
}
