# Enstream

Convert `Future` to `Stream` in a simple and lightweight manner.

Crate is fully compatible with `#![no_std]`.

## Example usage

```rust
#![feature(generic_associated_types, type_alias_impl_trait)]

use std::future::Future;

use enstream::{HandlerFn, Yielder, enstream};
use futures_util::{future::FutureExt, pin_mut, stream::StreamExt};

struct StreamState<'a> {
    val: &'a str
}

impl<'a> HandlerFn<'a, &'a str> for StreamState<'a> {
    type Fut<'yielder> = impl Future<Output = ()> + 'yielder
    where
        'a: 'yielder;

    fn call<'yielder>(self, mut yielder: Yielder<'yielder, &'a str>) -> Self::Fut<'yielder>
    where
        'a: 'yielder
    {
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
