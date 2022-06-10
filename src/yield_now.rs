use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pub enum YieldNow {
    Created,
    Yielded,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        match this {
            YieldNow::Created => {
                *this = YieldNow::Yielded;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            YieldNow::Yielded => Poll::Ready(()),
        }
    }
}
