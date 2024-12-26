use futures::{SinkExt, StreamExt, TryStreamExt};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use std::io;
use std::pin;

fn random(len: usize) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut data = vec![0; len];
    rng.fill_bytes(&mut data);
    data
}

#[tokio::test]
async fn test_random() {
    let file = tempfile::tempfile().unwrap();
    let (tx, rx) = crate::channel(file);

    let data = random(131101);

    let send = || async {
        let mut tx = pin::pin!(tx);
        for chunk in data.chunks(257) {
            tx.feed(chunk).await.unwrap();
        }
        SinkExt::<&[u8]>::close(&mut tx).await.unwrap();
    };

    let recv = || {
        let rx = rx.clone();
        async {
            let rx = rx
                .map_ok(|item| futures::stream::iter(item).map(Ok::<_, io::Error>))
                .try_flatten();
            let mut rx = pin::pin!(rx);
            for b in &*data {
                assert_eq!(rx.try_next().await.unwrap(), Some(*b));
            }
            assert_eq!(rx.try_next().await.unwrap(), None);
        }
    };

    futures::future::join3(recv(), send(), recv()).await;
    recv().await;
}
