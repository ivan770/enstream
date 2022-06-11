use std::{
    future::{self, Future},
    panic,
    task::{self, Poll},
};

use enstream::{HandlerFn, HandlerFnLifetime};
use futures_util::{stream::Stream, task::noop_waker_ref};

struct Handler;
impl HandlerFnLifetime<'_, Box<u32>> for Handler {
    type Fut = future::Ready<()>;
}
impl HandlerFn<Box<u32>> for Handler {
    fn call(
        self,
        mut yielder: enstream::Yielder<'_, Box<u32>>,
    ) -> <Self as HandlerFnLifetime<'_, Box<u32>>>::Fut {
        let mut cx = task::Context::from_waker(noop_waker_ref());

        let mut first = Box::pin(yielder.yield_item(Box::new(0)));
        assert_eq!(first.as_mut().poll(&mut cx), Poll::Pending);
        drop(first);

        let mut second = Box::pin(yielder.yield_item(Box::new(1)));
        let res = panic::catch_unwind(panic::AssertUnwindSafe(|| second.as_mut().poll(&mut cx)));
        assert!(res.is_err());

        future::ready(())
    }
}

#[test]
fn yield_multiple_panics() {
    let mut cx = task::Context::from_waker(noop_waker_ref());

    let mut stream = Box::pin(enstream::enstream(Handler));
    assert_eq!(stream.as_mut().poll_next(&mut cx), Poll::Ready(None));
}
