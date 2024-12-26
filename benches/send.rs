use criterion::Criterion;
use futures::SinkExt;
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use std::io::{BufWriter, Write};
use std::pin;

fn random(len: usize) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut data = vec![0; len];
    rng.fill_bytes(&mut data);
    data
}

fn send(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let data = random(1 << 24);
    let chunk_size = 1 << 12;

    c.bench_function("channel", |b| {
        b.to_async(&runtime).iter(|| async {
            let file = tempfile::tempfile().unwrap();
            let (tx, _) = file_channel::channel(file);
            let mut tx = pin::pin!(tx);
            for chunk in data.chunks(chunk_size) {
                tx.feed(chunk).await.unwrap();
            }
            SinkExt::<&[u8]>::close(&mut tx).await.unwrap();
        })
    });
    c.bench_function("std", |b| {
        b.iter(|| {
            let file = tempfile::tempfile().unwrap();
            let mut writer = BufWriter::new(file);
            for chunk in data.chunks(chunk_size) {
                writer.write_all(chunk).unwrap();
            }
            writer.flush().unwrap();
        })
    });
}

criterion::criterion_group!(benches, send);
criterion::criterion_main!(benches);
