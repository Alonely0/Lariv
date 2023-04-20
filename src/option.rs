use std::{
    fmt::Debug,
    mem::{replace, transmute, MaybeUninit},
    sync::{
        atomic::{AtomicBool, Ordering},
        RwLock, RwLockReadGuard, RwLockWriteGuard,
    },
};

/// Option with an atomic tag and interior synchronization.
pub struct AtomicOption<T> {
    tag: AtomicBool,
    value: RwLock<MaybeUninit<T>>,
}

/// Sets the tag to true after drop. Grabs a reference
/// to ensure the [`AtomicOption`] is not dropped.
pub struct SetGuard<'a, T> {
    guard: RwLockWriteGuard<'a, MaybeUninit<T>>,
    tag: &'a AtomicBool,
    written: bool,
}

#[allow(dead_code)]
impl<T> AtomicOption<T> {
    #[inline]
    pub const fn some(x: T) -> Self {
        Self {
            tag: AtomicBool::new(true),
            value: RwLock::new(MaybeUninit::new(x)),
        }
    }

    #[inline]
    pub const fn none() -> Self {
        Self {
            tag: AtomicBool::new(false),
            value: RwLock::new(MaybeUninit::uninit()),
        }
    }

    #[inline]
    pub fn try_set(&self) -> Option<SetGuard<'_, T>> {
        if let Ok(guard) = self.value.try_write() && !self.tag.load(Ordering::Acquire) {
            Some(SetGuard{guard, tag: &self.tag, written: false})
        } else {
            None
        }
    }

    #[inline]
    pub fn get(&self) -> Option<RwLockReadGuard<T>> {
        if let Ok(v) = self.value.read() && self.tag.load(Ordering::Acquire) {
            unsafe {
                Some(transmute::<RwLockReadGuard<MaybeUninit<T>>, RwLockReadGuard<T>>(
                    v,
                ))
            }
        } else {
            None
        }
    }

    #[inline]
    pub fn get_mut(&self) -> Option<RwLockWriteGuard<T>> {
        if let Ok(v) = self.value.write() && self.tag.load(Ordering::Acquire) {
            unsafe {
                Some(transmute::<RwLockWriteGuard<MaybeUninit<T>>, RwLockWriteGuard<T>>(
                    v,
                ))
            }
        } else {
            None
        }
    }

    #[inline]
    pub fn take(&self) -> Option<T> {
        if let Ok(mut guard) = self.value.write() && self.tag.load(Ordering::Acquire) {
            self.tag.store(false, Ordering::Release);
            Some(unsafe { replace(&mut *guard, MaybeUninit::<T>::uninit()).assume_init() })
        } else {
            None
        }
    }

    #[inline]
    pub fn empty(&self) {
        // Wait for the guard to get dropped
        let mut lock = unsafe { self.value.write().unwrap_unchecked() };
        // set tag to false, drop the inner if it was already written
        if self.tag.fetch_and(false, Ordering::AcqRel) {
            unsafe { lock.assume_init_drop() }
        }
    }
}

impl<'a, T> SetGuard<'a, T> {
    #[inline]
    pub fn write(&mut self, value: T) {
        (*self.guard).write(value);
        self.written = true;
    }
}

impl<T> Default for AtomicOption<T> {
    #[inline]
    fn default() -> Self {
        Self::none()
    }
}

impl<'a, T> Drop for SetGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        self.tag.store(self.written, Ordering::Release);
    }
}

impl<T> Debug for AtomicOption<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.get())
    }
}

impl<T> Drop for AtomicOption<T> {
    #[inline]
    fn drop(&mut self) {
        if *self.tag.get_mut() {
            unsafe { self.value.get_mut().unwrap_unchecked().assume_init_drop() }
        }
    }
}
