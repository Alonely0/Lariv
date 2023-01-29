#![allow(clippy::module_name_repetitions)]
use std::{
    fmt::Debug,
    marker::PhantomData,
    mem::MaybeUninit,
    sync::{
        atomic::{AtomicBool, Ordering},
        RwLock, RwLockWriteGuard,
    },
};

// this codebase is a bit dirty from the pre-rwlock implementation.
// ignore everything about an non-existent guard.

/// Option with an atomic tag and interior synchronization.
pub struct AtomicOption<T> {
    // guard: AtomicBool,
    tag: AtomicBool,
    value: RwLock<MaybeUninit<T>>,
    phantom: PhantomData<T>,
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
            // guard: AtomicBool::new(false),
            tag: AtomicBool::new(true),
            value: RwLock::new(MaybeUninit::new(x)),
            phantom: PhantomData,
        }
    }

    #[inline]
    pub const fn none() -> Self {
        Self {
            // guard: AtomicBool::new(false),
            tag: AtomicBool::new(false),
            value: RwLock::new(MaybeUninit::uninit()),
            phantom: PhantomData,
        }
    }

    /// The guard ensures the tag is set after initialization without
    /// having this function move the new value preemptively.
    #[inline]
    pub fn try_set(&self) -> Option<SetGuard<'_, T>> {
        // guard == true => return None
        // guard == false && tag == true => return None
        // guard == false && tag == false => set guard to true, return a guard
        // if !self.guard.fetch_or(true, Ordering::AcqRel) && !self.tag.load(Ordering::Acquire) {
        if let Ok(guard) = self.value.try_write() && !self.tag.load(Ordering::Acquire) {
            Some(SetGuard{guard, tag: &self.tag, written: false})
        } else {
            None
        }
    }

    #[inline]
    pub fn get(&self) -> Option<&RwLock<T>> {
        // self.wait_acq_guard();
        if self.tag.load(Ordering::Acquire) {
            // self.guard.store(false, Ordering::Release);
            Some(unsafe { &*(&self.value as *const RwLock<MaybeUninit<T>> as *const _) })
        } else {
            // self.guard.store(false, Ordering::Release);
            None
        }
    }

    // #[inline]
    // pub fn try_empty(&self) -> Result<(), ()> {
    //     // guard == true => nothing
    //     // guard == false && tag == false => nothing
    //     // guard == false && tag == true => set tag to none, drop inner
    //     if !self.guard.load(Ordering::Acquire) && self.tag.fetch_and(false, Ordering::AcqRel) {
    //         unsafe { (*self.value.get()).assume_init_drop() }
    //         Ok(())
    //     } else {
    //         Err(())
    //     }
    // }

    #[inline]
    pub fn empty(&self) {
        // Wait for guard to drop
        let mut lock = unsafe { self.value.write().unwrap_unchecked() };
        // set tag to false, drop the inner if it was already written
        if self.tag.fetch_and(false, Ordering::AcqRel) {
            unsafe { lock.assume_init_drop() }
        }
    }

    // #[inline]
    // fn wait_acq_guard(&self) {
    //     // while self
    //     // .guard
    //     // .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
    //     // .is_err()
    //     while self.guard.fetch_or(true, Ordering::AcqRel) {
    //         // println!("spinning at {}", line!());
    //         spin_loop();
    //     }
    // }
}

impl<'a, T> SetGuard<'a, T> {
    #[inline]
    pub fn write(&mut self, value: T) {
        (*self.guard).write(value);
        self.written = true;
    }
}

impl<T> const Default for AtomicOption<T> {
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
