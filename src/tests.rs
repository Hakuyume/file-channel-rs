use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use std::pin;
use std::sync::Arc;

pub(crate) fn random() -> Arc<[u8]> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut data = vec![0; 1 << 16];
    rng.fill_bytes(&mut data);
    data.into()
}

#[tokio::test]
async fn test_tempfile_random() {
    use futures::{AsyncReadExt, AsyncWriteExt};

    let (writer, reader) = crate::tempfile(tokio::runtime::Handle::current())
        .await
        .unwrap();
    let data = random();

    let read = || {
        let reader = reader.clone();
        let data = data.clone();
        tokio::spawn({
            async move {
                let mut reader = pin::pin!(reader);
                let mut buf = [0; 1];
                for b in &*data {
                    let n = reader.read(&mut buf).await.unwrap();
                    assert_eq!(n, 1);
                    assert_eq!(buf[0], *b);
                }
                let n = reader.read(&mut buf).await.unwrap();
                assert_eq!(n, 0);
            }
        })
    };

    let write = tokio::spawn({
        let data = data.clone();
        async move {
            let mut writer = pin::pin!(writer);
            for chunk in data.chunks(257) {
                writer.write_all(chunk).await.unwrap();
            }
        }
    });

    futures::future::try_join3(read(), write, read())
        .await
        .unwrap();
    read().await.unwrap();
}
