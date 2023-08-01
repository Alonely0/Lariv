use std::{
    intrinsics::unlikely,
    mem::ManuallyDrop,
    ptr::NonNull,
    sync::{RwLockReadGuard, RwLockWriteGuard},
};

use crate::{Epoch, Lariv, LarivNode, SharedItems};

impl<'a, T, E: Epoch> Lariv<T, E> {
    pub fn iter(&'a self) -> Iter<'a, T, E> {
        Iter {
            buf: self,
            current_node: unsafe {
                NonNull::new_unchecked(self.list.as_ref() as *const _ as *mut _)
            },
            next_index: 0,
        }
    }
    pub fn iter_mut(&'a self) -> IterMut<'a, T, E> {
        IterMut {
            current_node: unsafe {
                NonNull::new_unchecked(self.list.as_ref() as *const _ as *mut _)
            },
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
                    NonNull::new_unchecked(
                        (&*$x.current_node.as_ptr()).next.get()? as *const _ as *mut _
                    )
                };
                $x.next_index = 0;
            }
            ret = unsafe { &*(*$x.current_node.as_ptr()).ptr.as_ptr().add($x.next_index) }.$y();
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
            current_node: unsafe {
                NonNull::new_unchecked(self.list.as_ref() as *const _ as *mut _)
            },
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
    type Item = RwLockReadGuard<'a, T>;

    fn next(&mut self) -> Option<Self::Item> {
        iter!(self, get)
    }
}

impl<'a, T, E: Epoch> Iterator for IterMut<'a, T, E> {
    type Item = RwLockWriteGuard<'a, T>;

    fn next(&mut self) -> Option<Self::Item> {
        iter!(self, get_mut)
    }
}

impl<T, E: Epoch> Drop for IntoIter<T, E> {
    fn drop(&mut self) {
        // it's safe but miri hates it
        #[cfg(not(miri))]
        unsafe {
            drop((
                Box::from_raw(self.buf.shared.as_ptr() as *const _ as *mut SharedItems<T, E>),
                Box::from_raw(self.buf.list.as_ref() as *const _ as *mut LarivNode<T, E>),
            ));
        }
    }
}
