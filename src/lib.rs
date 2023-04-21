#![feature(vec_into_raw_parts)]
#![feature(core_intrinsics)]
#![feature(sync_unsafe_cell)]
#![feature(let_chains)]
#![feature(const_ptr_write)]
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
use once_cell::sync::OnceCell;

use option::AtomicOption;

mod iter;
mod option;
#[cfg(test)]
mod tests;

/// # Linked Atomic Random Insert Vector.
///
/// Lariv is a multithreaded data structure similar to a vector, with the exception of being able
/// to remove any elements at any index (not just the last one) without copying the posterior elements.
/// Lariv is lock-free, with a worst-case O(n) smart insert algorithm that tries to keep inserts at a fast
/// constant speed. Reallocations are wait-free, and lookups (needed for getting and removing) are O(1).
/// Even though Lariv is designed for short-lived data, it works on most multithreaded scenarios where a
/// vector-like data structure is viable.
pub struct Lariv<'a, T> {
    list: AliasableBox<LarivNode<'a, T>>, // linked list to buffers
    shared: &'a SharedItems<'a, T>,       // shared items across nodes
}

/// Node of the Linked Buffer
#[derive(Debug)]
struct LarivNode<'a, T> {
    ptr: AtomicPtr<AtomicOption<T>>,    // buffer start
    next: OnceCell<AliasableBox<Self>>, // linked list, next node (buffer extension)
    allocated: AtomicBool,              // set when the node has allocated.
    nth: usize,                         // number of buffer (used for global index)
    shared: &'a SharedItems<'a, T>,     // shared items across nodes
}

/// This stores both the node and the index of a element on a Lariv, and is the key for O(1)
/// lookups and having 128 bits indexes (needed for TPR, the parent project of Lariv).
#[derive(Copy, Clone, Debug)]
pub struct LarivIndex {
    node: u64,
    index: u64,
}

/// Variables shared between nodes
#[derive(Debug)]
struct SharedItems<'a, T> {
    head: SyncUnsafeCell<MaybeUninit<&'a LarivNode<'a, T>>>, // pointer to the first node. Set after initialization
    cursor: AtomicUsize,                                     // current index on current node
    cursor_ptr: AtomicPtr<LarivNode<'a, T>>,                 // current node of the list
    cap: usize,                                              // capacity (max elements)
    allocation_threshold: AtomicIsize, // set to 30% of the capacity after reaching the end
    nodes: AtomicUsize,                // nodes allocated. Used for calculating capacity
}

impl<'a, T> Lariv<'a, T> {
    /// Builds a new Lariv with a specific capacity of elements per node. For maximum speeds,
    /// this should be quite over-budgeted, though here the performance hit of allocating a new
    /// node is negligible compared to most data structures.
    #[must_use]
    pub fn new(buf_cap: usize) -> Self {
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
            head.as_ref() as *const LarivNode<'a, T> as *mut _,
            Ordering::Relaxed,
        );
        unsafe {
            (*shared_items.head.get()).write(&*(head.as_ref() as *const _));
        };

        // return
        Self {
            list: head,
            shared: shared_items,
        }
    }

    /// Zeroes the buffer, the same as a [`None`].
    #[inline(always)]
    const fn init_buf<V>(ptr: *mut V, cap: usize) {
        unsafe { write_bytes(ptr, 0, cap) };
    }

    /// Inserts a new element into the [`Lariv`] and returns its [`LarivIndex`].
    #[inline]
    pub fn push(&self, conn: T) -> LarivIndex {
        // call LarivNode::push() on the node currently on the cursor
        // if miri ever complains about a data race here, change this to SeqCst
        unsafe { &*self.shared.cursor_ptr.load(Ordering::Acquire) }.push(conn)
    }

    /// Gets an immutable reference to an element via its [`LarivIndex`]. While this is held,
    /// calls to [`get_mut`], [`remove`], and [`take`] with the same [`LarivIndex`] will block.
    /// This function will block if there are any held references to the same element.
    ///
    /// [`get_mut`]: Lariv::get_mut
    /// [`remove`]: Lariv::remove
    /// [`take`]: Lariv::take
    #[inline]
    pub fn get(&self, index: LarivIndex) -> Option<RwLockReadGuard<T>> {
        self.get_ptr(index).and_then(|p| unsafe { &*p }.get())
    }

    /// Gets a mutable reference to an element via its [`LarivIndex`]. While this is held,
    /// calls to [`get`], [`remove`], and [`take`] with the same [`LarivIndex`] will block.
    /// This function will block if there are any held references to the same element.
    ///
    /// [`get`]: Lariv::get
    /// [`remove`]: Lariv::remove
    /// [`take`]: Lariv::take
    #[inline]
    pub fn get_mut(&self, index: LarivIndex) -> Option<RwLockWriteGuard<T>> {
        self.get_ptr(index).and_then(|p| unsafe { &*p }.get_mut())
    }

    /// Removes an element from the Lariv, this is an optimized version of [`take`]. This
    /// function will block if there are any held references to the same element.
    ///
    /// [`take`]: Lariv::take
    #[inline]
    pub fn remove(&self, index: LarivIndex) {
        let Some(e) = self.get_ptr(index) else { return };
        unsafe { &*e }.empty();
        self.shared
            .allocation_threshold
            .fetch_sub(1, Ordering::AcqRel);
    }

    /// Removes an element from the Lariv and returns it. A more optimized version of this
    /// function which does not return the removed value is [`remove`]. This function will
    /// block if there are any held references to the same element.
    ///
    /// [`remove`]: Lariv::remove
    #[inline]
    pub fn take(&self, index: LarivIndex) -> Option<T> {
        self.shared
            .allocation_threshold
            .fetch_sub(1, Ordering::AcqRel);
        unsafe { &*self.get_ptr(index)? }.take()
    }

    #[must_use]
    #[inline]
    fn get_ptr(&self, mut li: LarivIndex) -> Option<*const AtomicOption<T>> {
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

impl<'a, T> LarivNode<'a, T> {
    fn new(
        ptr: *mut AtomicOption<T>,
        nth: usize,
        shared_items: &'a SharedItems<'a, T>,
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
    fn push(&self, element: T) -> LarivIndex {
        let mut node = self;
        // claim an index in the current node in the cursor
        let mut index = node.shared.cursor.fetch_add(1, Ordering::AcqRel);
        // avoid recursion
        loop {
            // check availability and write the value
            if likely(index < node.shared.cap) && let Some(mut pos) =
                unsafe { &*node.ptr.load(Ordering::Relaxed).add(index) }.try_set()
            {
                pos.write(element);
                break LarivIndex::new(node.nth, index)
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
                        unsafe { *(next as *const AliasableBox<LarivNode<'a, T>>).cast() },
                         Ordering::Release
                    );
                    node.shared.cursor.store(1, Ordering::Release);
                    index = 0;
                } else if node.shared.allocation_threshold.load(Ordering::Acquire) <= 0 {
                    node.shared.allocation_threshold.store(
                        node.calculate_allocate_threshold(),
                        Ordering::Release
                    );
                    node.shared.cursor_ptr.store(unsafe {
                        (*node.shared.head.get()).assume_init() as *const LarivNode<'a, T>
                    }.cast_mut(), Ordering::Release);
                    node.shared.cursor.store(1, Ordering::Release);
                    index = 0;
                } else if likely(!node.allocated.fetch_or(true, Ordering::Acquire)) {
                    break node.extend(element)
                }
            };
        }
    }

    #[cold]
    #[inline]
    fn extend(&self, first_element: T) -> LarivIndex {
        // allocate buffer
        let (ptr, _, cap) = Vec::with_capacity(self.shared.cap).into_raw_parts();
        Lariv::<T>::init_buf::<AtomicOption<T>>(ptr, cap);

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
        self.shared.allocation_threshold.store(0, Ordering::Release);
        self.shared.cursor_ptr.store(node_ptr, Ordering::Release);
        LarivIndex::new(nth, 0)
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

impl<'a, T> Drop for Lariv<'a, T> {
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
                self.shared as *const _ as *mut SharedItems<'a, T>,
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
        LarivIndex { node, index }
    }
}
