use std::{
    intrinsics::unlikely,
    mem::ManuallyDrop,
    ptr::NonNull,
    sync::{atomic::Ordering, RwLockReadGuard, RwLockWriteGuard},
};

use crate::{Epoch, Lariv, LarivNode, SharedItems};

impl<'a, 'b, T, E: Epoch> Lariv<'a, T, E> {
    pub fn iter(&'b self) -> Iter<'a, 'b, T, E> {
        Iter {
            buf: self,
            current_node: unsafe {
                NonNull::new_unchecked(self.list.as_ref() as *const _ as *mut _)
            },
            next_index: 0,
        }
    }
    pub fn iter_mut(&'b self) -> IterMut<'a, 'b, T, E> {
        IterMut {
            current_node: unsafe {
                NonNull::new_unchecked(self.list.as_ref() as *const _ as *mut _)
            },
            buf: self,
            next_index: 0,
        }
    }
}

pub struct IntoIter<'a, T, E: Epoch> {
    buf: ManuallyDrop<Lariv<'a, T, E>>,
    current_node: NonNull<LarivNode<'a, T, E>>,
    next_index: usize,
}

pub struct Iter<'a, 'b, T, E: Epoch> {
    buf: &'b Lariv<'a, T, E>,
    current_node: NonNull<LarivNode<'a, T, E>>,
    next_index: usize,
}

pub struct IterMut<'a, 'b, T, E: Epoch> {
    buf: &'b Lariv<'a, T, E>,
    current_node: NonNull<LarivNode<'a, T, E>>,
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
            ret = unsafe {
                &*(*$x.current_node.as_ptr())
                    .ptr
                    .load(Ordering::Relaxed)
                    .add($x.next_index)
            }
            .$y();
            $x.next_index += 1;
        }
        ret
    }};
}

impl<'a, T, E: Epoch> IntoIterator for Lariv<'a, T, E> {
    type Item = T;
    type IntoIter = IntoIter<'a, T, E>;

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

impl<'a, T, E: Epoch> Iterator for IntoIter<'a, T, E> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        iter!(self, take)
    }
}

impl<'a, 'b, T, E: Epoch> Iterator for Iter<'a, 'b, T, E> {
    type Item = RwLockReadGuard<'a, T>;

    fn next(&mut self) -> Option<Self::Item> {
        iter!(self, get)
    }
}

impl<'a, 'b, T, E: Epoch> Iterator for IterMut<'a, 'b, T, E> {
    type Item = RwLockWriteGuard<'a, T>;

    fn next(&mut self) -> Option<Self::Item> {
        iter!(self, get_mut)
    }
}

impl<'a, T, E: Epoch> Drop for IntoIter<'a, T, E> {
    fn drop(&mut self) {
        // it's safe but miri hates it
        #[cfg(not(miri))]
        unsafe {
            drop((
                Box::from_raw(self.buf.shared as *const _ as *mut SharedItems<'a, T, E>),
                Box::from_raw(self.buf.list.as_ref() as *const _ as *mut LarivNode<'a, T, E>),
            ));
        }
    }
}
