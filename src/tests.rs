use std::{sync::Mutex, thread::scope};

use crate::{Lariv, LarivIndex};

#[test]
pub fn general() {
    let lariv = Lariv::new(10);
    let buf = &lariv;
    scope(|s| {
        for i in 1..=100 {
            let li = LarivIndex::new(i / 10, (i - 1) % 10);
            s.spawn(move || buf.push(i));
            s.spawn(move || {
                buf.get(li);
            });
            s.spawn(move || {
                buf.get_mut(li);
            });
            s.spawn(move || buf.take(li));
            s.spawn(move || buf.remove(li));
        }
    });
}

#[test]
pub fn interleaved_read_write() {
    let lariv = Lariv::new(10);
    let buf = &lariv;
    scope(|s| {
        for i in 1..330 {
            s.spawn(move || buf.push(i.to_string()));
            s.spawn(
                move || {buf.get(LarivIndex::new(i / 10, (i - 1) % 10));}
            );
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
        for i in 1..=330 {
            s.spawn(move || {
                c.lock()
                    .unwrap()
                    .push((buf.push(i.to_string()), i.to_string()))
            });
        }
    });
    for (i, e) in r.get_mut().unwrap().iter() {
        assert_eq!(*e, *lariv.get(*i).unwrap());
        lariv.remove(*i)
    }
}

#[test]
pub fn into_iter() {
    let lariv = Lariv::new(10);
    let buf = &lariv;
    scope(|s| {
        for i in 1..=100usize {
            s.spawn(move || buf.push(i));
        }
    });
    assert_eq!(lariv.into_iter().count(), 100)
}

#[test]
pub fn iter() {
    let lariv = Lariv::new(10);
    let buf = &lariv;
    scope(|s| {
        for i in 1..=100usize {
            s.spawn(move || buf.push(i));
        }
    });
    assert_eq!(lariv.iter().count(), 100)
}

#[test]
pub fn iter_mut() {
    let lariv = Lariv::new(10);
    let buf = &lariv;
    scope(|s| {
        for i in 1..=100usize {
            s.spawn(move || buf.push(i));
        }
    });
    assert_eq!(lariv.iter_mut().count(), 100)
}

#[test]
#[should_panic]
pub fn deadlock_cap_threshold() {
    let _ = Lariv::<()>::new(3);
}

#[test]
fn readme_example() {
    // The Lariv is backed by a collection of buffers
    // distributed across nodes, and you need to specify
    // the amount of elements each one holds at most.
    let capacity = 50;
    let lariv = Lariv::new(capacity);
    let ptr = &lariv;
    scope(|s| {
        for i in 1..=330 {
            // Some processing that is pushed to the Lariv.
            // The insertion order is completely random,
            // if you need it to be sorted append the correct index
            // with the data, `i` on this case, and sort it later.
            // The alternative is having threads starve waiting on
            // a lock, which often is not ideal at best.
            s.spawn(move || ptr.push(i.to_string()));
        }
    });
    // Iterate the Lariv and do something with the data.
    for e in lariv {
        println!("{e}");
    }
}
