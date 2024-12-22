use super::buf::Buf;
use super::runtime::{spawn_blocking, SpawnBlocking};
use std::fs::File;
use std::future::Future;
use std::io::IoSliceMut;
use std::os::unix::fs::FileExt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use std::{cmp, io};

#[pin_project::pin_project]
#[derive(Clone)]
pub(crate) struct Reader {
    capacity: usize,
    file: Arc<File>,
    offset: u64,
    #[pin]
    state: State,
}

#[pin_project::pin_project(project = StateProj)]
enum State {
    Idle(Option<Buf>),
    Busy(#[pin] SpawnBlocking<(Buf, usize, Option<io::Error>)>),
}

impl Reader {
    pub(crate) fn with_capacity(capacity: usize, file: Arc<File>) -> Self {
        Self {
            capacity,
            file,
            offset: 0,
            state: State::Idle(None),
        }
    }

    pub(crate) fn offset(&self) -> u64 {
        self.offset
    }

    pub(crate) fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_vectored(cx, &mut [IoSliceMut::new(buf)])
    }

    pub(crate) fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        self.poll(cx, |buf| {
            let len = buf.len();
            for b in bufs {
                let (data, _) = buf.as_slices();
                let n = cmp::min(b.len(), data.len());
                b[..n].copy_from_slice(&data[..n]);
                b.advance(n);
                buf.consume(n);

                let (data, _) = buf.as_slices();
                let n = cmp::min(b.len(), data.len());
                b[..n].copy_from_slice(&data[..n]);
                b.advance(n);
                buf.consume(n);
            }
            len - buf.len()
        })
        .map_ok(Option::unwrap_or_default)
    }

    fn poll<F, T>(self: Pin<&mut Self>, cx: &mut Context<'_>, f: F) -> Poll<io::Result<Option<T>>>
    where
        F: FnOnce(&mut Buf) -> T,
    {
        let mut this = self.project();
        loop {
            match this.state.as_mut().project() {
                StateProj::Idle(buf) => {
                    let mut buf = buf
                        .take()
                        .unwrap_or_else(|| Buf::with_capacity(*this.capacity));
                    if buf.is_empty() {
                        let file = this.file.clone();
                        let offset = *this.offset;
                        let f = spawn_blocking(move || unsafe {
                            let (head, _) = buf.spare_capacity_mut();
                            // https://github.com/rust-lang/rust/issues/89517
                            match file.read_at(super::buf::slice_assume_init_mut(head), offset) {
                                Ok(n) => {
                                    buf.set_init(n);
                                    (buf, n, None)
                                }
                                Err(e) => (buf, 0, Some(e)),
                            }
                        });
                        this.state.set(State::Busy(f));
                    } else {
                        let output = f(&mut buf);
                        this.state.set(State::Idle(Some(buf)));
                        break Poll::Ready(Ok(Some(output)));
                    }
                }
                StateProj::Busy(f) => {
                    let (buf, n, e) = match ready!(f.poll(cx)) {
                        Ok((buf, n, e)) => (Some(buf), n, e),
                        Err(e) => (None, 0, Some(e)),
                    };
                    *this.offset += n as u64;
                    this.state.set(State::Idle(buf));
                    if let Some(e) = e {
                        break Poll::Ready(Err(e));
                    } else if n == 0 {
                        break Poll::Ready(Ok(None));
                    }
                }
            }
        }
    }
}

impl Clone for State {
    fn clone(&self) -> Self {
        match self {
            Self::Idle(buf) => Self::Idle(buf.clone()),
            Self::Busy(_) => Self::Idle(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::DEFAULT_BUF_SIZE;
    use rand::rngs::StdRng;
    use rand::{Rng, RngCore, SeedableRng};
    use std::future;
    use std::io::{self, BufWriter, Write};
    use std::pin::{self, Pin};
    use std::sync::Arc;

    async fn read(reader: &mut Pin<&mut super::Reader>, buf: &mut [u8]) -> io::Result<usize> {
        future::poll_fn(|cx| reader.as_mut().poll_read(cx, buf)).await
    }

    #[tokio::test]
    async fn test() {
        async fn assert(reader: &mut Pin<&mut super::Reader>, mut expected: &[u8]) {
            let mut buf = [0; 3];
            loop {
                let n = read(reader, &mut buf).await.unwrap();
                if n == 0 {
                    break;
                } else {
                    assert_eq!(buf[..n], expected[..n]);
                    expected = &expected[n..];
                }
            }
            assert!(expected.is_empty());
        }

        let file = Arc::new(tempfile::tempfile().unwrap());
        let mut writer = &*file;

        let mut reader_0 = pin::pin!(super::Reader::with_capacity(DEFAULT_BUF_SIZE, file.clone()));
        assert(&mut reader_0, b"").await;

        writer.write_all(b"hello").unwrap();
        writer.flush().unwrap();
        let mut reader_1 = pin::pin!(reader_0.clone());
        assert(&mut reader_1, b"hello").await;

        let mut reader_2 = pin::pin!(reader_1.clone());
        assert(&mut reader_1, b"").await;
        assert(&mut reader_2, b"").await;

        writer.write_all(b" world").unwrap();
        writer.flush().unwrap();
        let mut reader_3 = pin::pin!(reader_0.clone());
        assert(&mut reader_0, b"hello world").await;
        assert(&mut reader_1, b" world").await;
        assert(&mut reader_2, b" world").await;
        assert(&mut reader_3, b"hello world").await;
    }

    #[tokio::test]
    async fn test_random() {
        let file = Arc::new(tempfile::tempfile().unwrap());
        let size = 1 << 24;
        let rng = StdRng::seed_from_u64(42);

        let read = tokio::spawn({
            let file = file.clone();
            let mut rng = rng.clone();
            async move {
                let mut reader =
                    pin::pin!(super::Reader::with_capacity(DEFAULT_BUF_SIZE, file.clone()));
                let mut size = size;
                while size > 0 {
                    let mut buf = [0; 1];
                    let n = read(&mut reader, &mut buf).await.unwrap();
                    if n > 0 {
                        size -= 1;
                        assert_eq!(buf[0], rng.gen());
                    }
                }
            }
        });

        let write = tokio::task::spawn_blocking({
            let file = file.clone();
            let mut rng = rng.clone();
            move || {
                let mut writer = BufWriter::new(&*file);
                let mut size = size;
                let mut buf = vec![0; 1031];
                while size > 0 {
                    for b in &mut buf {
                        *b = rng.gen();
                    }
                    writer.write_all(&buf).unwrap();
                    size -= buf.len() as i64;
                }
                writer.flush().unwrap();
            }
        });

        futures::future::try_join(read, write).await.unwrap();
    }
}
