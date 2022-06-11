//! Enstream provides a way to convert [`Future`] to [`Stream`].
//!
//! # Example
//!
//! ```
//! #![feature(generic_associated_types, type_alias_impl_trait)]
//!
//! use std::future::Future;
//!
//! use enstream::{HandlerFn, Yielder, enstream};
//! use futures_util::{future::FutureExt, pin_mut, stream::StreamExt};
//!
//! struct StreamState<'a> {
//!     val: &'a str
//! }
//!
//! impl<'a> HandlerFn<'a, &'a str> for StreamState<'a> {
//!     type Fut<'yielder> = impl Future<Output = ()> + 'yielder
//!     where
//!         'a: 'yielder;
//!
//!     fn call<'yielder>(self, mut yielder: Yielder<'yielder, &'a str>) -> Self::Fut<'yielder>
//!     where
//!         'a: 'yielder
//!     {
//!         async move {
//!             yielder.yield_item(self.val).await;
//!         }
//!     }
//! }
//!
//! let owned = String::from("test");
//!
//! let stream = enstream(StreamState {
//!     val: &owned
//! });
//!
//! pin_mut!(stream);
//!
//! assert_eq!(stream.next().now_or_never().flatten(), Some("test"));
//! ```
//!
//! [`Stream`]: futures_core::stream::Stream

#![no_std]
#![feature(generic_associated_types)]

mod yield_now;

use core::{
    cell::UnsafeCell,
    future::Future,
    hint::unreachable_unchecked,
    marker::PhantomData,
    mem::MaybeUninit,
    pin::Pin,
    ptr::NonNull,
    task::{Context, Poll},
};

use futures_core::stream::{FusedStream, Stream};
use pin_project::pin_project;
use pinned_aliasable::Aliasable;
use yield_now::YieldNow;

/// [`Future`] generator that can be converted to [`Stream`].
pub trait HandlerFn<'scope, T: 'scope> {
    type Fut<'yielder>: Future<Output = ()> + 'yielder
    where
        'scope: 'yielder;

    /// Create new [`Future`] with the provided [`Yielder`] as a [`Stream`] item source.
    ///
    /// `'yielder` lifetime is defined inside of library internals,
    /// thus you are not allowed to use it to access outer scope elements.
    ///
    /// However, for those cases [`HandlerFn`] provides you with `'scope` lifetime,
    /// which is required to outlive `'yielder`.
    fn call<'yielder>(self, yielder: Yielder<'yielder, T>) -> Self::Fut<'yielder>
    where
        'scope: 'yielder;
}

/// [`Stream`] item yielder.
pub struct Yielder<'a, T>(NonNull<Option<T>>, PhantomData<&'a mut T>);

impl<'a, T> Yielder<'a, T> {
    /// Yield an item to stream.
    ///
    /// Each call to [`yield_item`] overrides the previous value
    /// yielded to stream.
    ///
    /// To ensure that the previous value is not overriden,
    /// you have to `await` the future returned by [`yield_item`].
    ///
    /// [`yield_item`]: Yielder::yield_item
    pub async fn yield_item(&mut self, val: T) {
        // Safety:
        //
        // Validity of internal NonNull pointer
        // is constrained by the lifetime of PhantomData,
        // which is defined by the lifetime of a Enstream struct.
        unsafe { self.0.as_ptr().write(Some(val)) }

        YieldNow::Created.await;
    }
}

// Safety:
//
// Yielder doesn't use any special thread semantics,
// so it's safe to Send it to another thread as long as you can Send T.
unsafe impl<'a, T: Send> Send for Yielder<'a, T> {}

// Safety:
//
// Since we don't provide any access to T
// we can make Yielder Sync even if T is not Sync.
unsafe impl<'a, T: Send> Sync for Yielder<'a, T> {}

#[pin_project(project = EnstreamStateProj)]
enum EnstreamState<G, F> {
    Gen(MaybeUninit<G>),
    Fut(#[pin] F),
    Completed,
}

#[pin_project]
struct Enstream<T, G, F> {
    #[pin]
    cell: Aliasable<UnsafeCell<Option<T>>>,

    #[pin]
    state: EnstreamState<G, F>,
}

impl<'yielder, 'scope: 'yielder, T: 'scope, G: 'scope> Stream
    for Enstream<T, G, <G as HandlerFn<'scope, T>>::Fut<'yielder>>
where
    G: HandlerFn<'scope, T>,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        // Safety: Pointer is guaranteed to be not null.
        let mut cell_pointer = unsafe { NonNull::new_unchecked(this.cell.as_ref().get().get()) };

        let poll_result = match this.state.as_mut().project() {
            EnstreamStateProj::Fut(fut) => fut.poll(cx),
            EnstreamStateProj::Gen(gen) => {
                // Safety: EnstreamState::Gen is always initialized with
                // valid future generator instance, that is read only
                // once, since after the initial read we replace EnstreamState::Gen
                // with EnstreamState::Fut or EnstreamState::Completed.
                let gen = unsafe { gen.assume_init_read() };

                // Since the generator is provided by user,
                // we have to protect ourselves from a panic
                // while the future is being initialized.
                this.state.set(EnstreamState::Completed);

                let fut = gen.call(Yielder(cell_pointer, PhantomData));
                this.state.set(EnstreamState::Fut(fut));

                match this.state.as_mut().project() {
                    EnstreamStateProj::Fut(fut) => fut.poll(cx),
                    // Safety: EnstreamState was set to Fut above.
                    _ => unsafe { unreachable_unchecked() },
                }
            }
            EnstreamStateProj::Completed => return Poll::Ready(None),
        };

        match poll_result {
            Poll::Ready(()) => {
                this.state.set(EnstreamState::Completed);
                Poll::Ready(None)
            }
            Poll::Pending => match unsafe { cell_pointer.as_mut() }.take() {
                Some(val) => Poll::Ready(Some(val)),
                None => Poll::Pending,
            },
        }
    }
}

impl<'yielder, 'scope: 'yielder, T: 'scope, G: 'scope> FusedStream
    for Enstream<T, G, <G as HandlerFn<'scope, T>>::Fut<'yielder>>
where
    G: HandlerFn<'scope, T>,
{
    fn is_terminated(&self) -> bool {
        matches!(self.state, EnstreamState::Completed)
    }
}

// Safety:
//
// Same as with Yielder, Enstream doesn't have any special thread-based semantics,
// thus it's safe to send it to another thread.
unsafe impl<T: Send, G: Send, F: Send> Send for Enstream<T, G, F> {}

// Safety:
//
// Enstream does not provide any way to access T, G or F via
// shared reference, thus it's safe to implement Sync for Enstream
// even if T, G or F don't implement it.
unsafe impl<T: Send, G: Send, F: Send> Sync for Enstream<T, G, F> {}

/// Create new [`Stream`] from the provided [`HandlerFn`].
pub fn enstream<'scope, T: 'scope, G: 'scope>(generator: G) -> impl FusedStream<Item = T> + 'scope
where
    G: HandlerFn<'scope, T>,
{
    Enstream {
        cell: Aliasable::new(UnsafeCell::new(None)),
        state: EnstreamState::Gen(MaybeUninit::new(generator)),
    }
}
