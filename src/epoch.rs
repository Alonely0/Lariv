use std::sync::{RwLockReadGuard, RwLockWriteGuard, atomic::Ordering};

use crate::{Lariv, LarivIndex};

#[const_trait]
pub trait Epoch: Copy {
    fn new(e: u64) -> Self;
    fn check(&self, e: u64) -> bool;
    fn update(&mut self);
}

/// It is a ZST, so it is effectively a zero-cost abstraction because it will get optimized away.
#[derive(Copy, Clone, Debug)]
pub struct NoEpoch;

/// Just a wrapper over [`u64`], for no good reason really. It works and it is more versatile so it stays.
#[repr(transparent)]
#[derive(Copy, Clone, Debug)]
pub struct LarivEpoch(pub u64);

impl const Epoch for LarivEpoch {
    #[inline]
    fn new(e: u64) -> Self {
        Self(e)
    }

    #[inline(always)]
    fn check(&self, e: u64) -> bool {
        self.0 == e
    }

    #[inline]
    fn update(&mut self) {
        self.0 += 1
    }
}

impl const Epoch for NoEpoch {
    #[inline]
    fn new(_e: u64) -> Self {
        Self
    }

    #[inline(always)]
    fn check(&self, _e: u64) -> bool {
        true
    }

    #[inline(always)]
    fn update(&mut self) {}
}

impl<'a, T> Lariv<'a, T, LarivEpoch> {
    /// Gets an immutable reference to an element via its [`LarivIndex`]. While this is held,
    /// calls to [`get_mut`], [`remove`], and [`take`] with the same [`LarivIndex`] will block.
    /// This function will block if there are any held references to the same element. This
    /// function will only return an element if it is of the same epoch of the index, i.e. it has
    /// not been replaced by a different element. Note that this function requires [`Lariv`] to be
    /// created with [`new_with_epoch`], as for optimization purposes epochs are opt-in.
    ///
    /// [`get_mut`]: Lariv::get_mut
    /// [`remove`]: Lariv::remove
    /// [`take`]: Lariv::take
    /// [`new_with_epoch`]: Lariv::new_with_epoch
    #[inline]
    pub fn get_with_epoch(&self, index: LarivIndex<LarivEpoch>) -> Option<RwLockReadGuard<T>> {
        self.get_ptr(index)
            .and_then(|p| unsafe { &*p }.get_with_epoch(index.epoch.0))
    }

    /// Gets a mutable reference to an element via its [`LarivIndex`]. While this is held,
    /// calls to [`get`], [`remove`], and [`take`] with the same [`LarivIndex`] will block.
    /// This function will block if there are any held references to the same element. Note
    /// that this function requires [`Lariv`] to be created with [`new_with_epoch`], as for
    /// optimization purposes epochs are opt-in.
    ///
    /// [`get`]: Lariv::get
    /// [`remove`]: Lariv::remove
    /// [`take`]: Lariv::take
    /// [`new_with_epoch`]: Lariv::new_with_epoch
    #[inline]
    pub fn get_mut_with_epoch(&self, index: LarivIndex<LarivEpoch>) -> Option<RwLockWriteGuard<T>> {
        self.get_ptr(index)
            .and_then(|p| unsafe { &*p }.get_mut_with_epoch(index.epoch.0))
    }

    /// Removes an element from the Lariv, ensuring it is the correct element. This is
    /// an optimized version of [`take`]. This function will block if there are any held
    /// references to the same element. Note that this function requires [`Lariv`] to be
    /// created with [`new_with_epoch`], as for optimization purposes epochs are opt-in.
    ///
    /// [`take`]: Lariv::take
    /// [`new_with_epoch`]: Lariv::new_with_epoch
    #[inline]
    pub fn remove_with_epoch(&self, index: LarivIndex<LarivEpoch>) {
        let Some(e) = self.get_ptr(index) else { return };
        unsafe { &*e }.empty_with_epoch(index.epoch.0);
        self.shared
            .allocation_threshold
            .fetch_sub(1, Ordering::AcqRel);
    }

    /// Removes an element from the Lariv and returns it, ensuring it is the correct element.
    /// A more optimized version of this function which does not return the removed value is
    /// [`remove`]. This function will block if there are any held references to the same element.
    /// Note that this function requires [`Lariv`] to be created with [`new_with_epoch`], as for
    /// optimization purposes epochs are opt-in.
    ///
    /// [`remove`]: Lariv::remove
    /// [`new_with_epoch`]: Lariv::new_with_epoch
    #[inline]
    pub fn take_with_epoch(&self, index: LarivIndex<LarivEpoch>) -> Option<T> {
        self.shared
            .allocation_threshold
            .fetch_sub(1, Ordering::AcqRel);
        unsafe { &*self.get_ptr(index)? }.take_with_epoch(index.epoch.0)
    }
}