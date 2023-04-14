#![feature(vec_into_raw_parts)]
#![feature(core_intrinsics)]
#![feature(sync_unsafe_cell)]
#![feature(let_chains)]
#![feature(const_trait_impl)]
#![feature(const_ptr_write)]
#![feature(strict_provenance)]

use std::{
    cell::SyncUnsafeCell,
    fmt::Debug,
    intrinsics::{likely, unlikely},
    mem::{align_of, transmute, ManuallyDrop, MaybeUninit},
    ops::Index,
    ptr::{invalid_mut, write_bytes},
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicUsize, Ordering},
        RwLock,
    },
};

use aliasable::prelude::*;
use once_cell::sync::OnceCell;

use option::AtomicOption;

mod option;
#[cfg(test)]
mod tests;

/// Linked Atomic Vector
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
    #[must_use]
    pub fn new(buf_cap: usize) -> Self {
        // tbh idk
        if buf_cap < 3 {
            panic!("For some reason buf_cap must be more than 3!")
        }

        // allocate
        let (ptr, len, cap) = Vec::with_capacity(buf_cap).into_raw_parts();
        Self::init_buf(ptr, cap);
        // create shared items.
        let shared_items: &'a _ = Box::leak(Box::new(SharedItems {
            head: SyncUnsafeCell::new(MaybeUninit::uninit()),
            cursor: AtomicUsize::new(len),
            cursor_ptr: AtomicPtr::new(invalid_mut(align_of::<LarivNode<'a, T>>())),
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
        unsafe { (*shared_items.head.get()).write(&*(head.as_ref() as *const _)) };

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

    #[inline]
    pub fn push(&self, conn: T) -> LarivIndex {
        // call LarivNode::push() on the node currently on the cursor
        // if miri ever complains about a data race here, change this to SeqCst
        unsafe { &*self.shared.cursor_ptr.load(Ordering::Acquire) }.push(conn)
    }

    #[must_use]
    #[inline]
    pub fn get(&self, index: LarivIndex) -> Option<&RwLock<T>> {
        self.traverse_get(index).and_then(|p| unsafe { &*p }.get())
    }

    #[inline]
    pub fn remove(&self, index: LarivIndex) {
        let Some(e) = self.traverse_get(index) else { return };
        unsafe { &*e }.empty();
        self.shared
            .allocation_threshold
            .fetch_sub(1, Ordering::AcqRel);
    }

    #[must_use]
    #[inline]
    fn traverse_get(&self, mut li: LarivIndex) -> Option<*const AtomicOption<T>> {
        let mut node = &self.list;
        if li.index as usize >= node.shared.cap {
            return None;
        };
        // traverse the node list and get the element
        while li.node > 0 {
            node = node.next.get()?;
            li.node -= 1
        }
        Some(unsafe { node.ptr.load(Ordering::Relaxed).add(li.index as usize) })
    }

    #[must_use]
    #[inline]
    pub fn cap(&self) -> usize {
        self.shared.nodes.load(Ordering::Acquire) * self.shared.cap
    }
}

impl<'a, T> Index<LarivIndex> for Lariv<'a, T> {
    type Output = RwLock<T>;
    #[inline]
    fn index(&self, index: LarivIndex) -> &Self::Output {
        self.get(index).expect("index out of bounds")
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
                index = node.shared.cursor.fetch_update(Ordering::AcqRel, Ordering::Acquire, |i| {
                    if i > node.shared.cap {
                        Some((i - 1) % node.shared.cap)
                    } else {
                        Some(i + 1)
                    }
                }).unwrap_or_else(|i| i);
                node = unsafe { &*node.shared.cursor_ptr.load(Ordering::Acquire) };
                if unlikely(index == 0) {
                    if let Some(next) = node.next.get() {
                        // traverse to the next node
                        node.shared.cursor_ptr.store(
                            unsafe { *(next as *const AliasableBox<LarivNode<'a, T>>).cast() },
                             Ordering::Release
                        );
                        node.shared.cursor.store(0, Ordering::Release);
                    } else if node.shared.allocation_threshold.load(Ordering::Acquire) <= 0 {
                        node.shared.allocation_threshold.store(
                            node.calculate_allocate_threshold(),
                            Ordering::Release
                        );
                        node.shared.cursor_ptr.store(unsafe {
                            (*node.shared.head.get()).assume_init() as *const LarivNode<'a, T>
                        }.cast_mut(), Ordering::Release);
                        node.shared.cursor.store(0, Ordering::Release);
                    } else if likely(!node.allocated.fetch_or(true, Ordering::Acquire)) {
                        break node.extend(element)
                    }
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
        let mut next = Some(&self.list);
        while let Some(node) = next {
            writeln!(f, "{:?}", unsafe {
                ManuallyDrop::new(Vec::<AtomicOption<T>>::from_raw_parts(
                    node.ptr.load(Ordering::Relaxed),
                    node.shared.cap,
                    node.shared.cap,
                ))
                .iter()
                .map(|x| x.get().and_then(|x| x.read().ok()))
                .collect::<Vec<_>>()
            })?;
            next = node.next.get();
        }
        write!(f, "")
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
            transmute([value.node.to_le_bytes(), value.index.to_le_bytes()])
        })
    }
}
