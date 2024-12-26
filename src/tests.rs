use futures::{SinkExt, StreamExt, TryStreamExt};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use std::io;
use std::pin;
use std::sync::Arc;

fn random() -> Arc<[u8]> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut data = vec![0; 1 << 16];
    rng.fill_bytes(&mut data);
    data.into()
}

#[tokio::test]
async fn test_tempfile_random() {
    let file = tempfile::tempfile().unwrap();
    let (tx, rx) = crate::channel(file);

    let data = random();

    let send = || {
        tokio::spawn({
            let data = data.clone();
            async move {
                let mut tx = pin::pin!(tx);
                for chunk in data.chunks(257) {
                    tx.send(chunk).await.unwrap();
                }
                SinkExt::<&[u8]>::close(&mut tx).await.unwrap();
            }
        })
    };

    let recv = || {
        let rx = rx.clone();
        let data = data.clone();
        tokio::spawn({
            async move {
                let rx = rx
                    .map_ok(|item| futures::stream::iter(item).map(Ok::<_, io::Error>))
                    .try_flatten();
                let mut rx = pin::pin!(rx);
                for b in &*data {
                    assert_eq!(rx.try_next().await.unwrap(), Some(*b));
                }
                assert_eq!(rx.try_next().await.unwrap(), None);
            }
        })
    };

    futures::future::try_join3(recv(), send(), recv())
        .await
        .unwrap();
    recv().await.unwrap();
}
