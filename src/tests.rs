use std::thread::scope;

use crate::{LarivIndex, Lariv};

#[test]
pub fn all_of_it() {
    let buffer = Lariv::new(10);
    let buf = &buffer;
    scope(|s| {
        for i in 1..=100 {
            let li = LarivIndex::new((i as f64 / 10.0).abs() as usize, i % 10);
            println!("{li:?}");
            s.spawn(move || buf.push(i));
            s.spawn(move || buf.get(li));
            s.spawn(move || buf.remove(li));
        }
    });
    println!("{buffer:?}");
}