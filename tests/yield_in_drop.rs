#![feature(generic_associated_types, type_alias_impl_trait)]

use std::{future::Future, task, task::Poll};

use enstream::{enstream, Yielder};
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
impl<'scope> enstream::HandlerFn<'scope, Box<u32>> for Handler {
    type Fut<'yielder> = impl Future<Output = ()> + 'yielder
        where
            'scope: 'yielder;

    fn call<'yielder>(self, yielder: Yielder<'yielder, Box<u32>>) -> Self::Fut<'yielder>
    where
        'scope: 'yielder,
    {
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
