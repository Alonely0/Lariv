use std::{
    fmt::{self, Debug},
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    ptr::{drop_in_place, replace, NonNull},
    sync::{
        atomic::{AtomicBool, Ordering},
        RwLock, RwLockReadGuard, RwLockWriteGuard,
    },
};

use crate::{epoch::LarivEpoch, Epoch};

pub(crate) type AtomicElement<T> = MaybeUninit<T>;

/// Option with an atomic tag and interior synchronization.
pub struct AtomicOptionTag<E: Epoch> {
    tag: AtomicBool,
    epoch: RwLock<E>,
}

/// Sets the tag to true after drop. Grabs a reference
/// to ensure the [`AtomicOption`] is not dropped.
pub struct SetGuard<'a, E: Epoch> {
    pub guard: RwLockWriteGuard<'a, E>,
    tag: &'a AtomicBool,
    written: bool,
}

pub struct Guard<T, G> {
    value: NonNull<T>,
    #[allow(dead_code)]
    guard: G,
}

#[allow(dead_code)]
impl<E: Epoch> AtomicOptionTag<E> {
    #[inline(always)]
    pub fn some() -> Self {
        Self {
            tag: AtomicBool::new(true),
            epoch: RwLock::new(E::new(0)),
        }
    }

    #[inline(always)]
    pub fn none() -> Self {
        Self {
            tag: AtomicBool::new(false),
            epoch: RwLock::new(E::new(0)),
        }
    }

    #[inline]
    pub fn try_set(&self) -> Option<SetGuard<'_, E>> {
        if let Ok(guard) = self.epoch.try_write() && !self.tag.load(Ordering::Acquire) {
            Some(SetGuard{guard, tag: &self.tag, written: false})
        } else {
            None
        }
    }
    pub fn get<T>(
        &'_ self,
        value: NonNull<AtomicElement<T>>,
    ) -> Option<Guard<T, RwLockReadGuard<'_, E>>> {
        if let Ok(e) = self.epoch.read() && self.tag.load(Ordering::Acquire) {
            Some(Guard { value: value.cast(), guard: e })
        } else {
            None
        }
    }

    #[inline]
    pub fn get_mut<T>(
        &self,
        value: NonNull<AtomicElement<T>>,
    ) -> Option<Guard<T, RwLockWriteGuard<'_, E>>> {
        if let Ok(e) = self.epoch.write() && self.tag.load(Ordering::Acquire) {
            Some(Guard { value: value.cast(), guard: e })
        } else {
            None
        }
    }

    #[inline]
    pub fn take<T>(&self, value: NonNull<AtomicElement<T>>) -> Option<T> {
        if let Ok(guard) = self.epoch.write() && self.tag.load(Ordering::Acquire) {
            self.tag.store(false, Ordering::Release);
            let e = unsafe { replace(value.as_ptr(), MaybeUninit::<T>::uninit()).assume_init() };
            drop(guard);
            Some(e)
        } else {
            None
        }
    }

    #[inline]
    pub fn empty<T>(&self, value: NonNull<AtomicElement<T>>) {
        // Wait for the guard to get dropped
        let lock = unsafe { self.epoch.write().unwrap_unchecked() };
        // set tag to false, drop the inner if it was already written
        if self.tag.fetch_and(false, Ordering::AcqRel) {
            unsafe { drop_in_place(value.as_ptr().cast::<T>()) }
        }
        drop(lock);
    }
}

impl AtomicOptionTag<LarivEpoch> {
    pub fn get_with_epoch<T>(
        &self,
        value: NonNull<AtomicElement<T>>,
        epoch: u64,
    ) -> Option<Guard<T, RwLockReadGuard<'_, LarivEpoch>>> {
        if let Ok(e) = self.epoch.read() && self.tag.load(Ordering::Acquire) && e.check(epoch) {
            Some(Guard { value: value.cast(), guard: e })
        } else {
            None
        }
    }

    #[inline]
    pub fn get_mut_with_epoch<T>(
        &self,
        value: NonNull<AtomicElement<T>>,
        epoch: u64,
    ) -> Option<Guard<T, RwLockWriteGuard<'_, LarivEpoch>>> {
        if let Ok(e) = self.epoch.write() && self.tag.load(Ordering::Acquire) && e.check(epoch) {
            Some(Guard { value: value.cast(), guard: e })
        } else {
            None
        }
    }

    #[inline]
    pub fn take_with_epoch<T>(&self, value: NonNull<AtomicElement<T>>, epoch: u64) -> Option<T> {
        if let Ok(mut guard) = self.epoch.write() && self.tag.load(Ordering::Acquire) && guard.check(epoch) {
            self.tag.store(false, Ordering::Release);
            guard.update();
            Some(unsafe { replace(value.as_ptr(), MaybeUninit::<T>::uninit()).assume_init() })
        } else {
            None
        }
    }

    #[inline]
    pub fn empty_with_epoch<T>(&self, value: NonNull<AtomicElement<T>>, epoch: u64) {
        // Wait for the guard to get dropped
        let lock = unsafe { self.epoch.write().unwrap_unchecked() };
        if lock.check(epoch) {
            // set tag to false, drop the inner if it was already written
            if self.tag.fetch_and(false, Ordering::AcqRel) {
                unsafe { drop_in_place(value.as_ptr().cast::<T>()) }
            }
        }
        drop(lock);
    }
}

impl<'a, E: Epoch> SetGuard<'a, E> {
    #[inline]
    pub fn write<T>(&mut self, ptr: NonNull<AtomicElement<T>>, value: T) {
        unsafe { ptr.as_ptr().write(MaybeUninit::new(value)) };
        self.guard.update();
        self.written = true;
    }
}

impl<E: Epoch> Default for AtomicOptionTag<E> {
    #[inline]
    fn default() -> Self {
        Self::none()
    }
}

impl<'a, E: Epoch> Drop for SetGuard<'a, E> {
    #[inline]
    fn drop(&mut self) {
        self.tag.store(self.written, Ordering::Release);
    }
}

impl<E: Epoch> Debug for AtomicOptionTag<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AtomicTag: {:?}", self.tag.load(Ordering::Acquire))
    }
}

impl<T, G: Deref> Deref for Guard<T, G> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.value.as_ref() }
    }
}

impl<T, G: DerefMut> DerefMut for Guard<T, G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.value.as_mut() }
    }
}

impl<T: fmt::Debug, G> fmt::Debug for Guard<T, G> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        unsafe { self.value.as_ref() }.fmt(f)
    }
}

impl<T: fmt::Display, G> fmt::Display for Guard<T, G> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        unsafe { self.value.as_ref() }.fmt(f)
    }
}

unsafe impl<T: Send, G: Send> Send for Guard<T, G> {}
unsafe impl<T: Sync, G: Sync> Sync for Guard<T, G> {}
