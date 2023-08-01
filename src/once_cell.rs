use std::debug_assert;
use std::mem::forget;
use std::ptr::null_mut;
use std::{
    mem::transmute,
    sync::atomic::{AtomicPtr, Ordering},
};

use aliasable::prelude::AliasableBox;

#[derive(Debug)]
#[repr(transparent)]
pub struct OnceAliasableBox<T> {
    inner: AtomicPtr<T>,
}

impl<T> OnceAliasableBox<T> {
    pub const fn new() -> OnceAliasableBox<T> {
        OnceAliasableBox {
            inner: AtomicPtr::new(null_mut()),
        }
    }

    pub fn get(&self) -> Option<&T> {
        let ptr = self.inner.load(Ordering::Acquire);
        if ptr.is_null() {
            return None;
        }
        Some(unsafe { &*ptr })
    }

    pub unsafe fn set_unchecked(&self, v: AliasableBox<T>) {
        debug_assert!(self.inner.load(Ordering::Acquire).is_null());
        self.inner
            .store(v.as_ref() as *const T as *mut T, Ordering::Release);
        forget(v);
    }
}

impl<T> Drop for OnceAliasableBox<T> {
    fn drop(&mut self) {
        let ptr = *self.inner.get_mut();
        if !ptr.is_null() {
            // safe but miri hates it.
            #[cfg(not(miri))]
            drop(unsafe { transmute::<*mut T, AliasableBox<T>>(ptr) })
        }
    }
}

unsafe impl<T: Sync + Send> Sync for OnceAliasableBox<T> {}