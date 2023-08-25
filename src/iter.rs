use std::{
    alloc::dealloc,
    intrinsics::unlikely,
    mem::ManuallyDrop,
    ptr::NonNull,
    sync::{RwLockReadGuard, RwLockWriteGuard},
};

use crate::{
    alloc::layout,
    option::{AtomicOptionTag, Guard},
    Epoch, Lariv, LarivNode,
};

impl<'a, T, E: Epoch> Lariv<T, E> {
    pub fn iter(&'a self) -> Iter<'a, T, E> {
        Iter {
            buf: self,
            current_node: self.list,
            next_index: 0,
        }
    }
    pub fn iter_mut(&'a self) -> IterMut<'a, T, E> {
        IterMut {
            current_node: self.list,
            buf: self,
            next_index: 0,
        }
    }
}

pub struct IntoIter<T, E: Epoch> {
    buf: ManuallyDrop<Lariv<T, E>>,
    current_node: NonNull<LarivNode<T, E>>,
    next_index: usize,
}

pub struct Iter<'a, T, E: Epoch> {
    buf: &'a Lariv<T, E>,
    current_node: NonNull<LarivNode<T, E>>,
    next_index: usize,
}

pub struct IterMut<'a, T, E: Epoch> {
    buf: &'a Lariv<T, E>,
    current_node: NonNull<LarivNode<T, E>>,
    next_index: usize,
}

macro_rules! iter {
    ($x:ident, $y:ident) => {{
        let mut ret = None;
        while ret.is_none() {
            if unlikely($x.next_index >= $x.buf.node_capacity()) {
                $x.current_node = unsafe {
                        (&*$x.current_node.as_ptr()).next.get()?.into()
                };
                $x.next_index = 0;
            }
            ret = unsafe {
                let node = &*$x.current_node.as_ptr();
                AtomicOptionTag::$y(
                    &*node.metadata_ptr().as_ptr().add($x.next_index),
                    NonNull::new_unchecked(node.data_ptr().as_ptr().add($x.next_index)),
                )
            };
            $x.next_index += 1;
        }
        ret
    }};
}

impl<T, E: Epoch> IntoIterator for Lariv<T, E> {
    type Item = T;
    type IntoIter = IntoIter<T, E>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            current_node: self.list,
            buf: ManuallyDrop::new(self),
            next_index: 0,
        }
    }
}

impl<T, E: Epoch> Iterator for IntoIter<T, E> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        iter!(self, take)
    }
}

impl<'a, T, E: Epoch> Iterator for Iter<'a, T, E> {
    type Item = Guard<T, RwLockReadGuard<'a, E>>;

    fn next(&mut self) -> Option<Self::Item> {
        iter!(self, get)
    }
}

impl<'a, T, E: Epoch> Iterator for IterMut<'a, T, E> {
    type Item = Guard<T, RwLockWriteGuard<'a, E>>;

    fn next(&mut self) -> Option<Self::Item> {
        iter!(self, get_mut)
    }
}

// safe but miri hates it (stacked borrows at it again, w/ tree borrows is fine)
impl<T, E: Epoch> Drop for IntoIter<T, E> {
    fn drop(&mut self) {
        let mut current_node = Some(unsafe { self.buf.list.as_ref() });
        let buf_cap = unsafe { self.buf.list.as_ref() }.get_shared().cap;
        unsafe {
            while let Some(node) = current_node {
                current_node = node.next.get();
                dealloc(node as *const _ as *mut u8, layout::<T, E>(buf_cap));
            }
            drop(Box::from_raw(self.buf.shared.as_ptr()))
        }
    }
}
