#![feature(type_alias_impl_trait)]

use std::{future::Future, task, task::Poll};

use enstream::{enstream, HandlerFn, HandlerFnLifetime, Yielder};
use futures_util::{stream::Stream, task::noop_waker_ref};

struct YieldsInDrop<'stream> {
    yielder: Yielder<'stream, Box<u32>>,
}

impl<'stream> Drop for YieldsInDrop<'stream> {
    fn drop(&mut self) {
        let mut cx = task::Context::from_waker(noop_waker_ref());
        let mut fut = Box::pin(self.yielder.yield_item(Box::new(0)));
        assert_eq!(fut.as_mut().poll(&mut cx), Poll::Pending);
    }
}

struct Handler;
type Fut<'yielder> = impl Future<Output = ()>;
impl<'yielder> HandlerFnLifetime<'yielder, Box<u32>> for Handler {
    type Fut = Fut<'yielder>;
}
impl HandlerFn<Box<u32>> for Handler {
    fn call(
        self,
        yielder: Yielder<'_, Box<u32>>,
    ) -> <Self as HandlerFnLifetime<'_, Box<u32>>>::Fut {
        async {
            let _yields_in_drop = YieldsInDrop { yielder };
            std::future::pending().await
        }
    }
}

#[test]
fn test() {
    let mut cx = task::Context::from_waker(noop_waker_ref());

    let mut stream = Box::pin(enstream(Handler));
    assert_eq!(stream.as_mut().poll_next(&mut cx), Poll::Pending);
    drop(stream);
}
