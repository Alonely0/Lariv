use criterion::{black_box, criterion_group, criterion_main, Criterion};
use dashmap::DashMap;
use lariv::{Lariv, LarivIndex};
use std::{mem::ManuallyDrop, thread::scope};

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("lariv", |b| b.iter(bench));
    c.bench_function("dashmap", |b| b.iter(bench2));
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);

fn bench() {
    let data = black_box(BigArray([0; 4096]));
    let buffer = black_box(ManuallyDrop::new(Lariv::<BigArray>::new(5000)));
    let buf = &buffer;
    scope(|s| {
        for i in 1..=10000 {
            let li = LarivIndex::new(i / 5000, (i % 5000) - 1);
            s.spawn(move || buf.push(data));
            s.spawn(move || {
                buf.get(li);
            });
            s.spawn(move || buf.remove(li));
        }
    });
}

fn bench2() {
    let data = black_box(BigArray([0; 4096]));
    let buffer = black_box(ManuallyDrop::new(DashMap::new()));
    let buf = &buffer;
    scope(|s| {
        for i in 1..10000 {
            s.spawn(move || buf.insert(i, data));
            s.spawn(move || {
                buf.get(&(i - 1));
            });
            s.spawn(move || buf.remove(&(i - 1)));
        }
    });
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct BigArray([u8; 4096]);
