#![feature(core_intrinsics)]
#![feature(let_chains)]
#![feature(const_alloc_error)]
#![cfg_attr(miri, allow(unused_imports))]
#![doc = include_str!("../README.md")]
#![allow(clippy::pedantic)]

use std::{
    alloc::dealloc,
    fmt::Debug,
    intrinsics::likely,
    mem::{needs_drop, transmute, MaybeUninit},
    ptr::{drop_in_place, NonNull},
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicUsize, Ordering},
        RwLockReadGuard, RwLockWriteGuard,
    },
};

use alloc::{allocate, data_offset, layout, metadata_offset, AddBytes};
pub use epoch::{Epoch, LarivEpoch, NoEpoch};

use once_cell::OncePtr;
use option::{AtomicElement, AtomicOptionTag, Guard};

mod alloc;
mod epoch;
mod iter;
mod once_cell;
mod option;

macro_rules! cast_mut {
    ($x:expr) => {
        $x as *const _ as *mut _
    };
}

/// # Linked Atomic Random Insert Vector.
///
/// Lariv is a multithreaded data structure similar to a vector, with the exception of being able
/// to remove any elements at any index (not just the last one) without copying the posterior elements.
/// Lariv is lock-free, with a worst-case O(n) smart insert algorithm that tries to keep inserts at a fast
/// constant speed. Reallocations are wait-free, and lookups (needed for getting and removing) are O(n/cap).
/// Even though Lariv is designed for short-lived data, it works on most multithreaded scenarios where a
/// vector-like data structure is viable.
///
/// Lariv has two modes, with and without epochs. The default, without, is the most performant one, but will
/// disable the `*_with_epoch` functions, which check that the element you access and/or delete is the same
/// that the one that was inserted when the [`LarivIndex`] was returned by the [`push`] function. For changing
/// between modes, see the [`new`] and [`new_with_epoch`] functions.
///
/// [`push`]: Lariv::push
/// [`new`]: Lariv::new
/// [`new_with_epoch`]: Lariv::new_with_epoch
pub struct Lariv<T, E: Epoch = NoEpoch> {
    list: NonNull<LarivNode<T, E>>,     // linked list to buffers
    shared: NonNull<SharedItems<T, E>>, // shared items across nodes
}

/// Node of the Linked Buffer
#[derive(Debug)]
struct LarivNode<T, E: Epoch> {
    next: OncePtr<Self>,                // linked list, next node (buffer extension)
    allocated: AtomicBool,              // set when the node has allocated.
    nth: usize,                         // number of buffer (used for global index)
    shared: NonNull<SharedItems<T, E>>, // shared items across nodes
}

/// This stores both the node and the index of an element on a Lariv instance.
#[derive(Copy, Clone, Debug)]
pub struct LarivIndex<E: Epoch = NoEpoch> {
    pub node: u64,
    pub index: u64,
    pub epoch: E,
}

/// Variables shared between nodes
#[derive(Debug)]
struct SharedItems<T, E: Epoch> {
    head: NonNull<LarivNode<T, E>>, // pointer to the first node. Set after initialization
    cursor: AtomicUsize,            // current index on current node
    cursor_ptr: AtomicPtr<LarivNode<T, E>>, // current node of the list
    cap: usize,                     // capacity (max elements)
    allocation_threshold: AtomicIsize, // set to 30% of the capacity after reaching the end
    nodes: AtomicUsize,             // nodes allocated. Used for calculating capacity
}

impl<T> Lariv<T> {
    /// Builds a new Lariv with a specific capacity of elements per node. For maximum speeds,
    /// this should be quite over-budgeted, though here the performance hit of allocating a new
    /// node is negligible compared to most data structures.
    #[must_use]
    pub fn new(buf_cap: usize) -> Lariv<T, NoEpoch> {
        Lariv::init(buf_cap)
    }

    /// Builds a new Lariv with a specific capacity of elements per node. For maximum speeds,
    /// this should be quite over-budgeted, though here the performance hit of allocating a new
    /// node is negligible compared to most data structures. This function creates a Lariv with
    /// epoch disabled, which increases performance and lowers memory usage. For creating a Lariv
    /// with epochs enabled, see [`new_with_epoch`].
    ///
    /// [`new_with_epoch`]: Lariv::new_with_epoch
    #[must_use]
    pub fn new_with_epoch(buf_cap: usize) -> Lariv<T, LarivEpoch> {
        Lariv::init(buf_cap)
    }

    #[must_use]
    fn init<E: Epoch>(buf_cap: usize) -> Lariv<T, E> {
        // tbh idk
        assert!(buf_cap > 3, "For some reason buf_cap must be more than 3!");

        // allocate
        let head = allocate::<T, E>(buf_cap);

        // create shared items.
        let shared_items = Box::leak(Box::new(SharedItems {
            head,
            cursor: AtomicUsize::new(0),
            cursor_ptr: AtomicPtr::new(head.as_ptr()),
            cap: buf_cap,
            allocation_threshold: AtomicIsize::new(0),
            nodes: AtomicUsize::new(1),
        }));

        // create head and set the shared pointer
        LarivNode::write_in_place(head, 0, shared_items);

        // return
        Lariv {
            list: head,
            shared: shared_items.into(),
        }
    }
}

impl<T, E: Epoch> Lariv<T, E> {
    /// Inserts a new element into the [`Lariv`] and returns its [`LarivIndex`].
    #[inline]
    pub fn push(&self, conn: T) -> LarivIndex<E> {
        // call LarivNode::push() on the node currently on the cursor
        // if miri ever complains about a data race here, change this to SeqCst
        unsafe {
            &*self
                .list
                .as_ref()
                .get_shared()
                .cursor_ptr
                .load(Ordering::Acquire)
        }
        .push(conn)
    }

    /// Gets an immutable reference to an element via its [`LarivIndex`]. While this is held,
    /// calls to [`get_mut`], [`remove`], and [`take`] with the same [`LarivIndex`] will block.
    /// This function will block if there are any held references to the same element. If the
    /// element inserted was removed and then replaced by another one, this function will
    /// access that new. For more information, check [`get_with_epoch`].
    ///
    /// [`get_mut`]: Lariv::get_mut
    /// [`remove`]: Lariv::remove
    /// [`take`]: Lariv::take
    /// [`get_with_epoch`]: Lariv::get_with_epoch
    #[inline]
    pub fn get<I: Epoch>(&self, index: LarivIndex<I>) -> Option<Guard<T, RwLockReadGuard<'_, E>>> {
        self.get_ptr(index).and_then(|(tag, e)| tag.get(e))
    }

    /// Gets a mutable reference to an element via its [`LarivIndex`]. While this is held,
    /// calls to [`get`], [`remove`], and [`take`] with the same [`LarivIndex`] will block.
    /// This function will block if there are any held references to the same element. If
    /// the element inserted was removed and then replaced by another one, this function
    /// will access that new. For more information, check [`get_mut_with_epoch`].
    ///
    /// [`get`]: Lariv::get
    /// [`remove`]: Lariv::remove
    /// [`take`]: Lariv::take
    /// [`get_mut_with_epoch`]: Lariv::get_mut_with_epoch
    #[inline]
    pub fn get_mut<I: Epoch>(
        &self,
        index: LarivIndex<I>,
    ) -> Option<Guard<T, RwLockWriteGuard<'_, E>>> {
        self.get_ptr(index).and_then(|(tag, e)| tag.get_mut(e))
    }

    /// Removes an element from the Lariv, this is an optimized version of [`take`]. This
    /// function will block if there are any held references to the same element. If the
    /// the element intended to be removed was removed beforehand and then replaced by
    /// another one, this function will remove that new one. For more information, check
    /// [`remove_with_epoch`].
    ///
    /// [`take`]: Lariv::take
    /// [`remove_with_epoch`]: Lariv::remove_with_epoch
    #[inline]
    pub fn remove<I: Epoch>(&self, index: LarivIndex<I>) {
        let Some((tag, e)) = self.get_ptr(index) else {
            return;
        };
        tag.empty(e);
        unsafe { self.list.as_ref() }
            .get_shared()
            .allocation_threshold
            .fetch_sub(1, Ordering::AcqRel);
    }

    /// Removes an element from the Lariv and returns it. A more optimized version of this
    /// function which does not return the removed value is [`remove`]. This function will
    /// block if there are any held references to the same element. If the the element
    /// intended to be taken was taken and/or removed beforehand and then replaced by
    /// another one, this function will take that new one. For more information, check
    /// [`take_with_epoch`].
    ///
    /// [`remove`]: Lariv::remove
    /// [`take_with_epoch`]: Lariv::take_with_epoch
    #[inline]
    pub fn take<I: Epoch>(&self, index: LarivIndex<I>) -> Option<T> {
        unsafe { self.list.as_ref() }
            .get_shared()
            .allocation_threshold
            .fetch_sub(1, Ordering::AcqRel);
        self.get_ptr(index).and_then(|(tag, e)| tag.take(e))
    }

    #[must_use]
    #[inline]
    fn get_ptr<I: Epoch>(
        &self,
        mut li: LarivIndex<I>,
    ) -> Option<(&AtomicOptionTag<E>, NonNull<AtomicElement<T>>)> {
        let shared = unsafe { self.list.as_ref().get_shared() };
        if li.index >= shared.cap as u64 || li.node >= shared.nodes.load(Ordering::Acquire) as u64 {
            return None;
        };
        let mut node = unsafe { self.list.as_ref() };
        while li.node > 0 {
            node = node.next.get()?;
            li.node -= 1;
        }
        Some(unsafe {
            (
                &*node.metadata_ptr().as_ptr().add(li.index as usize),
                NonNull::new_unchecked(node.data_ptr().as_ptr().add(li.index as usize)),
            )
        })
    }

    /// Returns the amount of elements any node can hold at most. This is the value given
    /// to the [`new`] function.
    ///
    /// [`new`]: Lariv::new
    #[inline]
    pub fn node_capacity(&self) -> usize {
        unsafe { self.list.as_ref() }.get_shared().cap
    }

    /// Returns the amount of nodes on the Lariv.
    #[inline]
    pub fn node_num(&self) -> usize {
        unsafe { self.list.as_ref() }
            .get_shared()
            .nodes
            .load(Ordering::Acquire)
    }

    /// Returns the amount of elements the Lariv can hold at most. This is equivalent to
    /// [`node_capacity`] multiplied by [`node_num`].
    ///
    /// [`node_capacity`]: Lariv::node_capacity
    /// [`node_num`]: Lariv::node_num
    #[inline]
    pub fn capacity(&self) -> usize {
        self.node_num() * self.node_capacity()
    }
}

impl<T, E: Epoch> LarivNode<T, E> {
    fn write_in_place(alloc: NonNull<Self>, nth: usize, shared_items: &SharedItems<T, E>) {
        unsafe {
            alloc.as_ptr().write(Self {
                next: OncePtr::new(),
                allocated: AtomicBool::new(false),
                nth,
                shared: shared_items.into(),
            })
        }
    }

    #[inline]
    fn push(&self, element: T) -> LarivIndex<E> {
        let mut node = self;
        let shared = self.get_shared();
        // claim an index in the current node in the cursor
        let mut index = shared.cursor.fetch_add(1, Ordering::AcqRel);
        'push: loop {
            // check availability and write the value
            if likely(index < shared.cap) && let Some(mut pos) =
                unsafe { &*node.metadata_ptr().as_ptr().add(index) }.try_set()
            {
                pos.write(unsafe { NonNull::new_unchecked(node.data_ptr().as_ptr().add(index)) }, element);
                break LarivIndex { node: node.nth as u64, index: index as u64, epoch: *pos.guard }
            }

            // I don't like how fetch_update codegens, it's kinda inefficient for this
            // because I'd be forced to have branches inside the closure, and I can
            // just optimize them manually.
            index = shared.cursor.load(Ordering::Acquire);
            'cas: loop {
                if likely(index < shared.cap) {
                    match shared.cursor.compare_exchange_weak(
                        index,
                        index + 1,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(x) => {
                            node = unsafe { &*shared.cursor_ptr.load(Ordering::Acquire) };
                            index = x;
                            continue 'push;
                        }
                        Err(next_prev) => index = next_prev,
                    }
                } else {
                    break 'cas;
                }
            }

            if let Some(next) = node.next.get() {
                // traverse to the next node
                node = next;
                index -= shared.cap;
                shared.cursor_ptr.store(cast_mut!(next), Ordering::Release);
                shared.cursor.store(index + 1, Ordering::Release);
                continue 'push;
            } else if shared.allocation_threshold.load(Ordering::Acquire) <= 0 {
                shared
                    .allocation_threshold
                    .store(node.calculate_allocate_threshold(), Ordering::Release);
                let head = shared.head.as_ptr();
                shared.cursor_ptr.store(head, Ordering::Release);
                shared.cursor.store(1, Ordering::Release);
                node = unsafe { &*head };
                index = 0;
                continue 'push;
            } else if !node.allocated.fetch_or(true, Ordering::AcqRel) {
                break 'push node.extend(element);
            }
        }
    }

    #[cold]
    #[inline]
    fn extend(&self, first_element: T) -> LarivIndex<E> {
        let shared = self.get_shared();
        // allocate buffer
        let alloc = allocate::<T, E>(shared.cap);
        let node = unsafe { alloc.as_ref() };
        let nth = self.nth + 1;
        Self::write_in_place(alloc, nth, unsafe { self.shared.as_ref() });
        // set first element
        unsafe {
            node.metadata_ptr().as_ptr().write(AtomicOptionTag::some());
            node.data_ptr()
                .as_ptr()
                .write(MaybeUninit::new(first_element));
        };
        // create node
        // set next
        unsafe { self.next.set_unchecked(alloc) };
        // update shared info
        shared.nodes.fetch_add(1, Ordering::AcqRel);
        shared
            .allocation_threshold
            .store(self.calculate_allocate_threshold(), Ordering::Release);
        shared.cursor_ptr.store(alloc.as_ptr(), Ordering::Release);
        shared.cursor.store(1, Ordering::Release);
        LarivIndex {
            node: nth as u64,
            index: 0,
            epoch: E::new(0),
        }
    }

    #[inline]
    fn calculate_allocate_threshold(&self) -> isize {
        // 30% of total capacity
        let shared = self.get_shared();
        ((shared.nodes.load(Ordering::Acquire) * shared.cap) as f64 * 0.3) as isize
    }

    #[inline(always)]
    fn get_shared(&self) -> &SharedItems<T, E> {
        unsafe { self.shared.as_ref() }
    }

    #[inline(always)]
    fn metadata_ptr(&self) -> NonNull<AtomicOptionTag<E>> {
        unsafe {
            NonNull::new_unchecked(
                (self as *const Self)
                    .add_bytes(metadata_offset::<T, E>(self.get_shared().cap))
                    .cast_mut()
                    .cast(),
            )
        }
    }

    #[inline(always)]
    fn data_ptr(&self) -> NonNull<AtomicElement<T>> {
        unsafe {
            NonNull::new_unchecked(
                (self as *const Self)
                    .add_bytes(data_offset::<T, E>(self.get_shared().cap))
                    .cast_mut()
                    .cast(),
            )
        }
    }
}

impl<T: Debug> Debug for Lariv<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[")?;
        let mut x = false;
        for e in self.iter() {
            if likely(!x) {
                write!(f, "{e:?}")?;
                x = true;
            } else {
                write!(f, ", {e:?}")?;
            }
        }

        write!(f, "]")
    }
}

// safe but miri hates it (stacked borrows at it again, w/ tree borrows is fine)
impl<T, E: Epoch> Drop for Lariv<T, E> {
    fn drop(&mut self) {
        let mut current_node = Some(unsafe { self.list.as_ref() });
        let buf_cap = unsafe { self.list.as_ref() }.get_shared().cap;
        unsafe {
            while let Some(node) = current_node {
                if needs_drop::<T>() {
                    for i in 0..buf_cap {
                        if *(*node.metadata_ptr().as_ptr().add(i)).tag.get_mut() {
                            drop_in_place(node.data_ptr().as_ptr().cast::<T>().add(i));
                        }
                    }
                }
                current_node = node.next.get();
                dealloc(node as *const _ as *mut u8, layout::<T, E>(buf_cap));
            }
            drop(Box::from_raw(self.shared.as_ptr()))
        }
    }
}

impl LarivIndex {
    #[inline]
    pub fn new(node: usize, index: usize) -> Self {
        Self {
            node: node as u64,
            index: index as u64,
            epoch: NoEpoch,
        }
    }
}

impl LarivIndex<LarivEpoch> {
    #[inline]
    pub fn new_with_epoch(node: usize, index: usize, epoch: usize) -> Self {
        Self {
            node: node as u64,
            index: index as u64,
            epoch: LarivEpoch(epoch as u64),
        }
    }
}

impl From<LarivIndex> for u128 {
    #[inline]
    fn from(value: LarivIndex) -> Self {
        u128::from_le_bytes(unsafe {
            transmute::<[[u8; 8]; 2], [u8; 16]>([
                value.node.to_le_bytes(),
                value.index.to_le_bytes(),
            ])
        })
    }
}

impl From<u128> for LarivIndex {
    #[inline]
    fn from(value: u128) -> Self {
        let [node, index] = unsafe { transmute::<[u8; 16], [u64; 2]>(u128::to_le_bytes(value)) };
        LarivIndex {
            node,
            index,
            epoch: NoEpoch,
        }
    }
}

#[cfg(test)]
mod tests;

unsafe impl<T: Send, E: Epoch> Send for Lariv<T, E> {}
unsafe impl<T: Sync, E: Epoch> Sync for Lariv<T, E> {}

unsafe impl<T: Send, E: Epoch> Send for LarivNode<T, E> {}
unsafe impl<T: Sync, E: Epoch> Sync for LarivNode<T, E> {}
