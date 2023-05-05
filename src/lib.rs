#![feature(vec_into_raw_parts)]
#![feature(core_intrinsics)]
#![feature(sync_unsafe_cell)]
#![feature(let_chains)]
#![feature(const_ptr_write)]
#![feature(const_trait_impl)]
#![feature(const_mut_refs)]
#![cfg_attr(miri, allow(unused_imports))]
#![doc = include_str!("../README.md")]

use std::{
    cell::SyncUnsafeCell,
    fmt::Debug,
    intrinsics::likely,
    mem::{transmute, MaybeUninit},
    ptr::{write_bytes, NonNull},
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicUsize, Ordering},
        RwLockReadGuard, RwLockWriteGuard,
    },
};

use aliasable::prelude::*;
use epoch::{Epoch, NoEpoch, LarivEpoch};
use once_cell::sync::OnceCell;

use option::AtomicOption;

mod iter;
mod option;
mod epoch;

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
 pub struct Lariv<'a, T, E: Epoch = NoEpoch> {
    list: AliasableBox<LarivNode<'a, T, E>>, // linked list to buffers
    shared: &'a SharedItems<'a, T, E>,       // shared items across nodes
}

/// Node of the Linked Buffer
#[derive(Debug)]
struct LarivNode<'a, T, E: Epoch> {
    ptr: AtomicPtr<AtomicOption<T, E>>, // buffer start
    next: OnceCell<AliasableBox<Self>>, // linked list, next node (buffer extension)
    allocated: AtomicBool,              // set when the node has allocated.
    nth: usize,                         // number of buffer (used for global index)
    shared: &'a SharedItems<'a, T, E>,  // shared items across nodes
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
struct SharedItems<'a, T, E: Epoch> {
    head: SyncUnsafeCell<MaybeUninit<&'a LarivNode<'a, T, E>>>, // pointer to the first node. Set after initialization
    cursor: AtomicUsize,                                        // current index on current node
    cursor_ptr: AtomicPtr<LarivNode<'a, T, E>>,                 // current node of the list
    cap: usize,                                                 // capacity (max elements)
    allocation_threshold: AtomicIsize, // set to 30% of the capacity after reaching the end
    nodes: AtomicUsize,                // nodes allocated. Used for calculating capacity
}

impl<'a, T> Lariv<'a, T> {
    /// Builds a new Lariv with a specific capacity of elements per node. For maximum speeds,
    /// this should be quite over-budgeted, though here the performance hit of allocating a new
    /// node is negligible compared to most data structures.
    #[must_use]
    pub fn new(buf_cap: usize) -> Lariv<'a, T, NoEpoch> {
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
    pub fn new_with_epoch(buf_cap: usize) -> Lariv<'a, T, LarivEpoch> {
        Lariv::init(buf_cap)
    }

    #[must_use]
    fn init<E: Epoch>(buf_cap: usize) -> Lariv<'a, T, E> {
        // tbh idk
        if buf_cap <= 3 {
            panic!("For some reason buf_cap must be more than 3!")
        }

        // allocate
        let (ptr, len, cap) = Vec::with_capacity(buf_cap).into_raw_parts();
        Self::init_buf(ptr, cap);
        // create shared items.
        let shared_items: &'a _ = Box::leak(Box::new(SharedItems {
            head: SyncUnsafeCell::new(MaybeUninit::uninit()),
            cursor: AtomicUsize::new(len),
            cursor_ptr: AtomicPtr::new(NonNull::dangling().as_ptr()),
            cap,
            allocation_threshold: AtomicIsize::new(0),
            nodes: AtomicUsize::new(1),
        }));

        // create head and set the shared pointer
        let head = LarivNode::new(ptr, 0, shared_items);
        shared_items.cursor_ptr.store(
            head.as_ref() as *const LarivNode<'a, T, E> as *mut _,
            Ordering::Relaxed,
        );
        unsafe {
            (*shared_items.head.get()).write(&*(head.as_ref() as *const _));
        };

        // return
        Lariv {
            list: head,
            shared: shared_items,
        }
    }

    /// Zeroes the buffer, the same as a [`None`].
    #[inline(always)]
    const fn init_buf<V>(ptr: *mut V, cap: usize) {
        unsafe { write_bytes(ptr, 0, cap) };
    }
}

impl<'a, T, E: Epoch> Lariv<'a, T, E> {
    /// Inserts a new element into the [`Lariv`] and returns its [`LarivIndex`].
    #[inline]
    pub fn push(&self, conn: T) -> LarivIndex<E> {
        // call LarivNode::push() on the node currently on the cursor
        // if miri ever complains about a data race here, change this to SeqCst
        unsafe { &*self.shared.cursor_ptr.load(Ordering::Acquire) }.push(conn)
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
    pub fn get<I: Epoch>(&self, index: LarivIndex<I>) -> Option<RwLockReadGuard<T>> {
        self.get_ptr(index).and_then(|p| unsafe { &*p }.get())
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
    pub fn get_mut<I: Epoch>(&self, index: LarivIndex<I>) -> Option<RwLockWriteGuard<T>> {
        self.get_ptr(index).and_then(|p| unsafe { &*p }.get_mut())
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
        let Some(e) = self.get_ptr(index) else { return };
        unsafe { &*e }.empty();
        self.shared
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
        self.shared
            .allocation_threshold
            .fetch_sub(1, Ordering::AcqRel);
        unsafe { &*self.get_ptr(index)? }.take()
    }

    #[must_use]
    #[inline]
    fn get_ptr<I: Epoch>(&self, mut li: LarivIndex<I>) -> Option<*const AtomicOption<T, E>> {
        if li.index as usize >= self.shared.cap
            || li.node as usize >= self.shared.nodes.load(Ordering::Acquire)
        {
            return None;
        };
        let mut node = &self.list;
        while li.node > 0 {
            node = node.next.get()?;
            li.node -= 1
        }
        Some(unsafe { node.ptr.load(Ordering::Relaxed).add(li.index as usize) })
    }

    /// Returns the amount of elements any node can hold at most. This is the value given
    /// to the [`new`] function.
    ///
    /// [`new`]: Lariv::new
    #[inline]
    pub fn node_capacity(&self) -> usize {
        self.shared.cap
    }

    /// Returns the amount of nodes on the Lariv.
    #[inline]
    pub fn node_num(&self) -> usize {
        self.shared.nodes.load(Ordering::Acquire)
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

impl<'a, T, E: Epoch> LarivNode<'a, T, E> {
    fn new(
        ptr: *mut AtomicOption<T, E>,
        nth: usize,
        shared_items: &'a SharedItems<'a, T, E>,
    ) -> AliasableBox<Self> {
        AliasableBox::from_unique(UniqueBox::new(Self {
            ptr: AtomicPtr::new(ptr),
            next: OnceCell::new(),
            allocated: AtomicBool::new(false),
            nth,
            shared: shared_items,
        }))
    }
    #[inline]
    fn push(&self, element: T) -> LarivIndex<E> {
        let mut node = self;
        // claim an index in the current node in the cursor
        let mut index = node.shared.cursor.fetch_add(1, Ordering::AcqRel);
        // avoid recursion
        loop {
            // check availability and write the value
            if likely(index < node.shared.cap) && let Some((mut pos, epoch)) =
                unsafe { &*node.ptr.load(Ordering::Relaxed).add(index) }.try_set()
            {
                pos.write(element);
                break LarivIndex { node: node.nth as u64, index: index as u64, epoch }
            } else {
                // ask for the next index, checking if it's the last one in the buffer
                node = unsafe { &*node.shared.cursor_ptr.load(Ordering::Acquire) };
                let i = node.shared.cursor.load(Ordering::Acquire);
                if i > node.shared.cap {
                    index = (i - 1) % node.shared.cap;
                } else {
                    index = node.shared.cursor.fetch_add(1, Ordering::AcqRel);
                    continue
                }
                if let Some(next) = node.next.get() {
                    // traverse to the next node
                    node.shared.cursor_ptr.store(
                        unsafe { *(next as *const AliasableBox<LarivNode<'a, T, E>>).cast() },
                         Ordering::Release
                    );
                    node.shared.cursor.store(1, Ordering::Release);
                    node = next;
                    index = 0;
                } else if node.shared.allocation_threshold.load(Ordering::Acquire) <= 0 {
                    node.shared.allocation_threshold.store(
                        node.calculate_allocate_threshold(),
                        Ordering::Release
                    );
                    let head = unsafe{ (*node.shared.head.get()).assume_init() as *const LarivNode<'a, T, E> };
                    node.shared.cursor_ptr.store(head.cast_mut(), Ordering::Release);
                    node.shared.cursor.store(1, Ordering::Release);
                    node = unsafe { &*head };
                    index = 0;
                } else if likely(!node.allocated.fetch_or(true, Ordering::Acquire)) {
                    break node.extend(element)
                }
            };
        }
    }

    #[cold]
    #[inline]
    fn extend(&self, first_element: T) -> LarivIndex<E> {
        // allocate buffer
        let (ptr, _, cap) = Vec::with_capacity(self.shared.cap).into_raw_parts();
        Lariv::<T>::init_buf::<AtomicOption<T, E>>(ptr, cap);

        // set first element
        unsafe { *ptr = AtomicOption::some(first_element) };
        // create node
        let nth = self.nth + 1;
        let node = Self::new(ptr, nth, self.shared);
        let node_ptr = node.as_ref() as *const _ as *mut _;
        // set next
        unsafe { self.next.set(node).unwrap_unchecked() };
        // update shared info
        self.shared.nodes.fetch_add(1, Ordering::AcqRel);
        self.shared
            .allocation_threshold
            .store(self.calculate_allocate_threshold(), Ordering::Release);
        self.shared.cursor_ptr.store(node_ptr, Ordering::Release);
        LarivIndex { node: nth as u64, index: 0, epoch: E::new(0) }
    }

    #[inline]
    fn calculate_allocate_threshold(&self) -> isize {
        // 30% of total capacity
        ((self.shared.nodes.load(Ordering::Acquire) * self.shared.cap) as f64 * 0.3) as isize
    }
}

impl<'a, T> Debug for Lariv<'a, T>
where
    T: Debug,
{
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

impl<'a, T, E: Epoch> Drop for Lariv<'a, T, E> {
    fn drop(&mut self) {
        let mut next = Some(&self.list);
        while let Some(node) = next {
            unsafe {
                // the vec handles the drop, which should be
                // `std::ptr::drop_in_place` on each element
                // and then free the allocation.
                drop(Vec::from_raw_parts(
                    node.ptr.load(Ordering::Relaxed),
                    node.shared.cap,
                    node.shared.cap,
                ));
            };
            next = node.next.get();
        }
        // it's safe but miri hates it
        #[cfg(not(miri))]
        unsafe {
            drop(Box::from_raw(
                self.shared as *const _ as *mut SharedItems<'a, T, E>,
            ));
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
