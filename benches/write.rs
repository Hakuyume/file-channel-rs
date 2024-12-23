use criterion::Criterion;
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use std::io::Write;
use std::pin;

fn random(len: usize) -> Box<[u8]> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut data = vec![0; len];
    rng.fill_bytes(&mut data);
    data.into()
}

fn write(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let data = random(1 << 24);
    let chunk_size = 1 << 12;

    c.bench_function("write", |b| {
        use futures::AsyncWriteExt;

        b.to_async(&runtime).iter(|| async {
            let (writer, _) = file_channel::tempfile(&runtime).await.unwrap();
            let mut writer = pin::pin!(writer);
            for chunk in data.chunks(chunk_size) {
                writer.write_all(chunk).await.unwrap();
            }
            writer.flush().await.unwrap();
        })
    });
    c.bench_function("write-std", |b| {
        b.iter(|| {
            let file = tempfile::tempfile().unwrap();
            let mut writer = &file;
            for chunk in data.chunks(chunk_size) {
                writer.write_all(chunk).unwrap();
            }
            writer.flush().unwrap();
        })
    });
    c.bench_function("write-tokio", |b| {
        use tokio::io::AsyncWriteExt;

        b.to_async(&runtime).iter(|| async {
            let file = tempfile::tempfile().unwrap();
            let mut writer = tokio::io::BufWriter::new(tokio::fs::File::from_std(file));
            for chunk in data.chunks(chunk_size) {
                writer.write_all(chunk).await.unwrap();
            }
            writer.flush().await.unwrap();
        })
    });
}

criterion::criterion_group!(benches, write);
criterion::criterion_main!(benches);
