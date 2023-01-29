use criterion::{black_box, criterion_group, criterion_main, Criterion};
use dashmap::DashMap;
use lariv::{Lariv, LarivIndex};
use std::thread::scope;

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("lariv", |b| b.iter(bench));
    c.bench_function("dashmap", |b| b.iter(bench2));
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);

fn bench() {
    let buffer = black_box(Lariv::new(black_box(5000)));
    let buf = &buffer;
    scope(|s| {
        let li = LarivIndex::new(1, 0);
        for i in 1..=10000 {
            s.spawn(move || buf.push(i));
            s.spawn(move || buf.get(li));
            s.spawn(move || buf.remove(li));
        }
    });
    // println!("{buffer:?}");
}

fn bench2() {
    let buffer = black_box(DashMap::new());
    let buf = &buffer;
    scope(|s| {
        for i in 1..10000 {
            s.spawn(move || buf.insert(i, i));
            s.spawn(move || buf.get(&(i - 1)).map(black_box));
            s.spawn(move || buf.remove(&(i - 1)));
        }
    });
}
