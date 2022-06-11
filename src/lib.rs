//! Enstream provides a way to convert [`Future`] to [`Stream`].
//!
//! # Example
//!
//! ```
//! #![feature(type_alias_impl_trait)]
//!
//! use std::future::Future;
//!
//! use enstream::{HandlerFn, HandlerFnLifetime, Yielder, enstream};
//! use futures_util::{future::FutureExt, pin_mut, stream::StreamExt};
//!
//! struct StreamState<'a> {
//!     val: &'a str
//! }
//!
//! // A separate type alias is used to work around TAIT bugs
//! type Fut<'yielder, 'a: 'yielder> = impl Future<Output = ()>;
//!
//! impl<'yielder, 'a> HandlerFnLifetime<'yielder, &'a str> for StreamState<'a> {
//!     type Fut = Fut<'yielder, 'a>;
//! }
//!
//! impl<'a> HandlerFn<&'a str> for StreamState<'a> {
//!     fn call<'yielder>(
//!         self,
//!         mut yielder: Yielder<'yielder, &'a str>,
//!     ) -> <Self as HandlerFnLifetime<'yielder, &'a str>>::Fut {
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

mod yield_now;

use core::{
    cell::UnsafeCell,
    future::Future,
    hint::unreachable_unchecked,
    marker::PhantomData,
    pin::Pin,
    ptr::NonNull,
    task::{Context, Poll},
};

use futures_core::stream::{FusedStream, Stream};
use pin_project::pin_project;
use pinned_aliasable::Aliasable;
use yield_now::YieldNow;

/// Associated types of a [`HandlerFn`] with the specific lifetime of the yielder applied.
///
/// This must be implemented for any lifetime `'yielder` in order to implement [`HandlerFn`].
pub trait HandlerFnLifetime<'yielder, T, ImplicitBounds: Sealed = Bounds<Yielder<'yielder, T>>> {
    /// The future type, as returned by [`HandlerFn::call`].
    type Fut: Future<Output = ()>;
}

/// [`Future`] generator that can be converted to [`Stream`].
pub trait HandlerFn<T>: for<'yielder> HandlerFnLifetime<'yielder, T> {
    /// Create new [`Future`] with the provided [`Yielder`] as a [`Stream`] item source.
    fn call(self, yielder: Yielder<'_, T>) -> <Self as HandlerFnLifetime<'_, T>>::Fut;
}

/// [`Stream`] item yielder.
pub struct Yielder<'a, T>(NonNull<Option<T>>, PhantomData<&'a mut T>);

impl<'a, T> Yielder<'a, T> {
    /// Yield an item to stream.
    ///
    /// # Panics
    ///
    /// Panics if an item has already been yielded in this polling of the stream future. This is
    /// hard to trigger unless you're intentionally misuing the API by calling `poll` functions
    /// manually.
    pub async fn yield_item(&mut self, val: T) {
        // Safety:
        //
        // Validity of internal NonNull pointer
        // is constrained by the lifetime of PhantomData,
        // which is defined by the lifetime of a Enstream struct.
        let slot = unsafe { self.0.as_mut() };

        assert!(slot.is_none(), "called `yield_item` twice in one poll");

        *slot = Some(val);

        YieldNow::Created.await;
    }
}

// Safety:
//
// Yielder doesn't use any special thread semantics,
// so it's safe to Send it to another thread as long as you can Send T.
unsafe impl<'a, T: Send> Send for Yielder<'a, T> {}

// Safety: You can do nothing with an `&Yielder<'a, T>`.
unsafe impl<'a, T> Sync for Yielder<'a, T> {}

#[pin_project(
    project = EnstreamStateProj,
    project_replace = EnstreamStateProjReplace,
)]
enum EnstreamState<G, F> {
    Gen(G),
    Fut(#[pin] F),
    Completed,
}

#[pin_project]
struct Enstream<T, G, F> {
    // This field must come before `cell` so that it is dropped before `cell`, to prevent the
    // future ending up with a dangling reference.
    #[pin]
    state: EnstreamState<G, F>,

    #[pin]
    cell: Aliasable<UnsafeCell<Option<T>>>,
}

impl<'yielder, T, G> Stream for Enstream<T, G, <G as HandlerFnLifetime<'yielder, T>>::Fut>
where
    G: HandlerFn<T>,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        // Safety: Pointer is guaranteed to be not null.
        let mut cell_pointer = unsafe { NonNull::new_unchecked(this.cell.as_ref().get().get()) };

        let poll_result = match this.state.as_mut().project() {
            EnstreamStateProj::Fut(fut) => fut.poll(cx),
            EnstreamStateProj::Gen(..) => {
                // Since the generator is provided by user,
                // we have to protect ourselves from a panic
                // while the future is being initialized.
                let gen = match this
                    .state
                    .as_mut()
                    .project_replace(EnstreamState::Completed)
                {
                    EnstreamStateProjReplace::Gen(gen) => gen,
                    // Safety: EnstreamState was checked as Gen above
                    _ => unsafe { unreachable_unchecked() },
                };

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

impl<'yielder, T, G> FusedStream for Enstream<T, G, <G as HandlerFnLifetime<'yielder, T>>::Fut>
where
    G: HandlerFn<T>,
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

// Safety: The only thing that an `&Enstream` allows is calling `is_terminated`, but that does not
// access any of `T`, `G` or `F`.
unsafe impl<T, G, F> Sync for Enstream<T, G, F> {}

/// Create new [`Stream`] from the provided [`HandlerFn`].
pub fn enstream<T, G>(generator: G) -> impl FusedStream<Item = T>
where
    G: HandlerFn<T>,
{
    Enstream {
        cell: Aliasable::new(UnsafeCell::new(None)),
        state: EnstreamState::Gen(generator),
    }
}

mod private {
    pub trait Sealed {}
    pub struct Bounds<T>(T);
    impl<T> Sealed for Bounds<T> {}
}
use private::{Bounds, Sealed};
