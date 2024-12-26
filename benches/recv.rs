use criterion::Criterion;
use futures::{SinkExt, TryStreamExt};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::pin;

fn random(len: usize) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut data = vec![0; len];
    rng.fill_bytes(&mut data);
    data
}

fn recv(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let data = random(1 << 24);

    c.bench_function("channel", |b| {
        let file = tempfile::tempfile().unwrap();
        let rx = runtime.block_on(async {
            let (tx, rx) = file_channel::channel(file);
            let mut tx = pin::pin!(tx);
            tx.send(&data).await.unwrap();
            rx
        });
        b.to_async(&runtime).iter(|| async {
            rx.clone().try_collect::<Vec<_>>().await.unwrap();
        })
    });
    c.bench_function("std", |b| {
        let file = tempfile::tempfile().unwrap();
        let mut writer = BufWriter::new(file);
        writer.write_all(&data).unwrap();

        let file = writer.into_inner().unwrap();
        let mut reader = BufReader::new(file);
        b.iter(|| {
            reader.seek(SeekFrom::Start(0)).unwrap();
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf).unwrap();
        })
    });
}

criterion::criterion_group!(benches, recv);
criterion::criterion_main!(benches);
