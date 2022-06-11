# Enstream

Convert `Future` to `Stream` in a simple and lightweight manner.

Crate is fully compatible with `#![no_std]`.

## Example usage

```rust
#![feature(type_alias_impl_trait)]

use std::future::Future;

use enstream::{HandlerFn, HandlerFnLifetime, Yielder, enstream};
use futures_util::{future::FutureExt, pin_mut, stream::StreamExt};

struct StreamState<'a> {
    val: &'a str
}

// A separate type alias is used to work around TAIT bugs
type Fut<'yielder, 'a: 'yielder> = impl Future<Output = ()>;

impl<'yielder, 'a> HandlerFnLifetime<'yielder, &'a str> for StreamState<'a> {
    type Fut = Fut<'yielder, 'a>;
}

impl<'a> HandlerFn<&'a str> for StreamState<'a> {
    fn call<'yielder>(
        self,
        mut yielder: Yielder<'yielder, &'a str>,
    ) -> <Self as HandlerFnLifetime<'yielder, &'a str>>::Fut {
        async move {
            yielder.yield_item(self.val).await;
        }
    }
}

let owned = String::from("test");

let stream = enstream(StreamState {
    val: &owned
});

pin_mut!(stream);

assert_eq!(stream.next().now_or_never().flatten(), Some("test"));
```
