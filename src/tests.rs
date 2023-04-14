use std::{
    sync::{
        Mutex,
    },
    thread::scope,
};

use crate::{Lariv, LarivIndex};

#[test]
pub fn general() {
    let lariv = Lariv::new(10);
    let buf = &lariv;
    scope(|s| {
        for i in 1..=100 {
            let li = LarivIndex::new(i / 10, i - 1);
            s.spawn(move || buf.push(i));
            s.spawn(move || buf.get(li));
            s.spawn(move || buf.remove(li));
        }
    });
}

#[test]
pub fn correctness() {
    let lariv = Lariv::new(10);
    let buf = &lariv;
    let mut r = Mutex::new(Vec::<(LarivIndex, String)>::with_capacity(100));
    let c = &r;
    scope(|s| {
        for i in 1..=100 {
            s.spawn(move || c.lock().unwrap().push((buf.push(i.to_string()), i.to_string())));
        }
    });
    for (i, e) in r.get_mut().unwrap().iter() {
        assert_eq!(*e, *lariv.get(*i).unwrap().read().unwrap());
        lariv.remove(*i)
    }
}
