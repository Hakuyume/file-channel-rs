#[cfg(feature = "futures-std")]
const _: () = {
    use crate::runtime::SpawnBlocking;
    use crate::{Reader, Writer};
    use std::io::{self, IoSlice, IoSliceMut};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    impl<R> futures::io::AsyncRead for Reader<R>
    where
        R: SpawnBlocking,
    {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            self.poll_read_vectored(cx, &mut [IoSliceMut::new(buf)])
        }
        fn poll_read_vectored(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bufs: &mut [IoSliceMut<'_>],
        ) -> Poll<io::Result<usize>> {
            self.poll_read_vectored(cx, bufs)
        }
    }

    impl<R> futures::io::AsyncWrite for Writer<R>
    where
        R: SpawnBlocking,
    {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.poll_write_vectored(cx, &[IoSlice::new(buf)])
        }
        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.poll_flush(cx)
        }
        fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.poll_close(cx)
        }
        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bufs: &[IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            self.poll_write_vectored(cx, bufs)
        }
    }
};
