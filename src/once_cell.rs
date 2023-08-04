use std::debug_assert;
use std::ptr::{null_mut, NonNull};
use std::sync::atomic::{AtomicPtr, Ordering};

#[derive(Debug)]
#[repr(transparent)]
pub struct OncePtr<T> {
    inner: AtomicPtr<T>,
}

impl<T> OncePtr<T> {
    pub const fn new() -> OncePtr<T> {
        Self {
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

    pub unsafe fn set_unchecked(&self, v: NonNull<T>) {
        debug_assert!(self.inner.load(Ordering::Acquire).is_null());
        self.inner.store(v.as_ptr(), Ordering::Release);
    }
}

unsafe impl<T: Send> Send for OncePtr<T> {}
unsafe impl<T: Sync> Sync for OncePtr<T> {}
