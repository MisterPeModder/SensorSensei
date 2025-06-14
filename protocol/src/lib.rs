#![cfg_attr(not(test), no_std)]
#![allow(async_fn_in_trait)]

pub mod app;
pub mod codec;
pub mod link;
pub mod phy;

/// Testing utilities
#[cfg(test)]
pub(crate) mod test {
    use core::future::Future;
    use std::{
        pin::Pin,
        task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
    };

    const NOOP_RAW_WAKER_VTABLE: RawWakerVTable =
        RawWakerVTable::new(|_| NOOP_RAW_WAKER, |_| {}, |_| {}, |_| {});
    const NOOP_RAW_WAKER: RawWaker = RawWaker::new(core::ptr::null(), &NOOP_RAW_WAKER_VTABLE);
    const NOOP_WAKER: Waker = unsafe { Waker::from_raw(NOOP_RAW_WAKER) };

    pub trait RunBlockingExt: Future {
        /// Evaluates this future by spin blocking, not quite energy-efficient.
        fn run_blocking(mut self) -> Self::Output
        where
            Self: Sized,
        {
            let waker = &NOOP_WAKER;
            let mut this: Pin<&mut Self> = unsafe { Pin::new_unchecked(&mut self) };
            let mut cx = Context::from_waker(waker);

            loop {
                if let Poll::Ready(res) = this.as_mut().poll(&mut cx) {
                    break res;
                }
                core::hint::spin_loop();
            }
        }
    }

    impl<F: Future> RunBlockingExt for F {}
}
