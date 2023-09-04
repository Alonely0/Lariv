//! Notice to mariners: Do not ever touch this. If you absolutely have to,
//! use the code in std::alloc as a reference and NEVER deviate from it.
//! Then, test with miri until she's happy. You have been warned.
//!
//! # Safety
//! Check std::alloc.
use std::{
    alloc::{alloc_zeroed, handle_alloc_error, Layout},
    ptr::NonNull,
};

use crate::{
    option::{AtomicElement, AtomicOptionTag},
    Epoch, LarivNode,
};

macro_rules! ok_or_err {
    ($x:expr, $layout:expr) => {
        match $x {
            Ok(x) => x,
            Err(_) => handle_alloc_error($layout),
        }
    };
}

macro_rules! some_or_err {
    ($x:expr, $layout:expr) => {
        match $x {
            Some(x) => x,
            None => handle_alloc_error($layout),
        }
    };
}

#[inline(always)]
pub(crate) const fn node_layout<T, E: Epoch>() -> Layout {
    Layout::new::<LarivNode<T, E>>()
}
#[inline]
pub(crate) const fn metadata_layout<E: Epoch>(buf_cap: usize) -> Layout {
    let layout = Layout::new::<AtomicOptionTag<E>>();
    ok_or_err!(
        Layout::from_size_align(
            some_or_err!(layout.size().checked_mul(buf_cap), layout),
            layout.align(),
        ),
        layout
    )
}

#[inline]
pub(crate) const fn data_layout<T>(buf_cap: usize) -> Layout {
    let layout = Layout::new::<AtomicElement<T>>();
    let len = layout.size();
    let padded_size = len + {
        let align = layout.align();
        let len_rounded_up = len.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1);
        len_rounded_up.wrapping_sub(len)
    };
    let data = ok_or_err!(
        Layout::from_size_align(
            some_or_err!(padded_size.checked_mul(buf_cap), layout),
            layout.align()
        ),
        layout
    );

    unsafe {
        Layout::from_size_align_unchecked(
            data.size() + {
                let align = data.align();
                let len = data.size();

                let len_rounded_up =
                    len.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1);
                len_rounded_up.wrapping_sub(len)
            },
            data.align(),
        )
    }
}

#[inline]
pub(crate) const fn metadata_offset<T, E: Epoch>(buf_cap: usize) -> usize {
    let node = node_layout::<T, E>();
    let next = metadata_layout::<E>(buf_cap);
    let pad = {
        let align = next.align();
        let len = node.size();
        (len.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1)).wrapping_sub(len)
    };

    node.size() + pad
}

#[inline]
pub(crate) const fn data_offset<T, E: Epoch>(buf_cap: usize) -> usize {
    metadata_offset::<T, E>(buf_cap) + {
        let metadata = metadata_layout::<E>(buf_cap);
        let data = data_layout::<T>(buf_cap);
        let pad = {
            let align = data.align();
            let len = metadata.size();
            let len_rounded_up = len.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1);
            len_rounded_up.wrapping_sub(len)
        };

        some_or_err!(metadata.size().checked_add(pad), data)
    }
}

pub(crate) const fn layout<T, E: Epoch>(buf_cap: usize) -> Layout {
    let node = node_layout::<T, E>();
    let metadata = metadata_layout::<E>(buf_cap);
    let prev = ok_or_err!(
        Layout::from_size_align(
            some_or_err!(
                some_or_err!(
                    node.size().checked_add({
                        let align = metadata.align();
                        let len = node.size();

                        let len_rounded_up =
                            len.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1);
                        len_rounded_up.wrapping_sub(len)
                    }),
                    node
                )
                .checked_add(metadata.size()),
                metadata
            ),
            if node.align() > metadata.align() {
                node.align()
            } else {
                metadata.align()
            }
        ),
        metadata
    );
    let data = data_layout::<T>(buf_cap);
    let new_align = if prev.align() > data.align() {
        prev.align()
    } else {
        data.align()
    };
    let pad = {
        let align = data.align();
        let len = prev.size();

        let len_rounded_up = len.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1);
        len_rounded_up.wrapping_sub(len)
    };

    let offset = some_or_err!(prev.size().checked_add(pad), data);
    let new_size = some_or_err!(offset.checked_add(data.size()), data);

    ok_or_err!(Layout::from_size_align(new_size, new_align), data)
}

#[inline(always)]
pub(crate) fn allocate<T, E: Epoch>(buf_cap: usize) -> NonNull<LarivNode<T, E>> {
    let layout = layout::<T, E>(buf_cap);
    some_or_err!(NonNull::new(unsafe { alloc_zeroed(layout).cast() }), layout)
}

pub(crate) trait AddBytes<T> {
    /// # Safety
    /// pointer + `bytes` must not overflow.
    unsafe fn add_bytes(self, bytes: usize) -> Self;
}

impl<T> AddBytes<T> for *const T {
    #[inline(always)]
    unsafe fn add_bytes(self, bytes: usize) -> Self {
        self.cast::<u8>().add(bytes).cast::<T>()
    }
}

impl<T> AddBytes<T> for *mut T {
    #[inline(always)]
    unsafe fn add_bytes(self, bytes: usize) -> Self {
        self.cast::<u8>().add(bytes).cast::<T>()
    }
}
