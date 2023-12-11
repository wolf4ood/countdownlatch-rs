//! This library provide an implementation of an async [`CountDownLatch`],
//! which keeps a counter syncronized via [`Lock`][async-lock::Lock] in it's internal state and allows tasks to wait until
//! the counter reaches zero.
//!
//! # Example
//! ```rust,no_run
//! use wait_for_me::CountDownLatch;
//! use smol::{self,Task};
//! fn main() -> Result<(), Box<std::error::Error>> {
//!    smol::run(async {
//!         let latch = CountDownLatch::new(1);
//!         let latch1 = latch.clone();
//!         Task::spawn(async move {
//!             latch1.count_down().await;
//!         }).detach();
//!         latch.wait().await;
//!         Ok(())
//!    })
//!
//!}
//! ```
//!
//! With timeout
//!
//! ```rust,no_run
//! use wait_for_me::CountDownLatch;
//! use smol::{Task,Timer};
//! use std::time::Duration;
//! #[smol_potat::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!    let latch = CountDownLatch::new(10);
//!    for _ in 0..10 {
//!        let latch1 = latch.clone();
//!        Task::spawn(async move {
//!            Timer::after(Duration::from_secs(3)).await;
//!            latch1.count_down().await;
//!        }).detach();
//!    }
//!    let result = latch.wait_for(Duration::from_secs(1)).await;
//!
//!    assert_eq!(false,result);
//!
//!    Ok(())
//!}
//!```
//!

use futures;
use futures::future::Either;
use futures_timer::Delay;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use async_lock::{Mutex};
use async_lock::futures::{LockArc};

struct CountDownState {
    count: usize,
    wakers: Vec<Waker>,
}

impl CountDownLatch {
    /// Creates a new [`CountDownLatch`] with a given count.
    pub fn new(count: usize) -> CountDownLatch {
        CountDownLatch {
            state: Arc::new(Mutex::new(CountDownState {
                count,
                wakers: vec![],
            })),
        }
    }

    /// Returns the current count.
    pub async fn count(&self) -> usize {
        let state = self.state.lock().await;
        state.count
    }

    /// Cause the current task to wait until the counter reaches zero
    pub fn wait(&self) -> impl Future<Output = ()> {
        WaitFuture {
            latch: self.clone(),
            state: None,
        }
    }

    /// Cause the current task to wait until the counter reaches zero with timeout.
    ///
    /// If the specified timeout elapesed `false` is retured. Otherwise `true`.
    pub async fn wait_for(&self, timeout: Duration) -> bool {
        let delay = Delay::new(timeout);
        match futures::future::select(delay, self.wait()).await {
            Either::Left(_) => false,
            Either::Right(_) => true,
        }
    }

    /// Decrement the counter of one unit. If the counter reaches zero all the waiting tasks are released.
    pub async fn count_down(&self) {
        let mut state = self.state.lock().await;

        match state.count {
            1 => {
                state.count -= 1;
                for waker in state.wakers.drain(..) {
                    waker.wake();
                }
            }
            n @ _ if n > 0 => {
                state.count -= 1;
            }
            _ => {}
        };
    }
}

struct WaitFuture {
    latch: CountDownLatch,
    state: Option<Box<LockArc<CountDownState>>>,
}

impl Future for WaitFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match self.state.take() {
                Some(mut state) => {
                    let mut guard =
                        futures::ready!(unsafe { Pin::new_unchecked(state.as_mut()) }.poll(cx));
                    if guard.count > 0 {
                        for waker in guard.wakers.iter() {
                            if waker.will_wake(cx.waker()) {
                                return Poll::Pending
                            }
                        }
                        guard.wakers.push(cx.waker().clone());
                        return Poll::Pending;
                    } else {
                        return Poll::Ready(());
                    }
                }
                None => {
                    self.state = Some(Box::new(self.latch.state.lock_arc()));
                }
            }
        }
    }
}

/// A synchronization primitive that allows one or more tasks to wait until the given counter reaches zero.
/// This is an async port of [CountDownLatch](https://docs.oracle.com/javase/7/docs/api/java/util/concurrent/CountDownLatch.html) in Java.
#[derive(Clone)]
pub struct CountDownLatch {
    state: Arc<Mutex<CountDownState>>,
}

#[cfg(test)]
mod tests {
    use super::CountDownLatch;
    use futures_executor::LocalPool;
    use futures_util::task::SpawnExt;
    use std::time::Duration;

    #[test]
    fn countdownlatch_test() {
        let mut pool = LocalPool::new();

        let spawner = pool.spawner();
        let latch = CountDownLatch::new(2);
        let latch1 = latch.clone();
        spawner
            .spawn(async move { latch1.count_down().await })
            .unwrap();

        let latch2 = latch.clone();
        spawner
            .spawn(async move { latch2.count_down().await })
            .unwrap();

        let latch3 = latch.clone();
        spawner
            .spawn(async move {
                latch3.wait().await;
            })
            .unwrap();

        spawner
            .spawn(async move {
                latch.wait().await;
            })
            .unwrap();

        pool.run();
    }

    #[test]
    fn countdownlatch_pre_wait_test() {
        let mut pool = LocalPool::new();

        let spawner = pool.spawner();
        let latch = CountDownLatch::new(1);

        let latch1 = latch.clone();
        spawner
            .spawn(async move { latch1.wait().await })
            .unwrap();

        spawner
            .spawn(async move { latch.count_down().await })
            .unwrap();

        pool.run();
    }

    #[test]
    fn countdownlatch_concurrent_test() {
        let mut pool = LocalPool::new();

        let spawner = pool.spawner();
        let latch = CountDownLatch::new(100);

        for _ in 0..200 {
            let latch1 = latch.clone();
            spawner
                .spawn(async move { latch1.count_down().await })
                .unwrap();
        }

        for _ in 0..100 {
            let latch1 = latch.clone();
            spawner.spawn(async move { latch1.wait().await }).unwrap();
        }

        pool.run();
    }

    #[test]
    fn countdownlatch_no_wait_test() {
        let mut pool = LocalPool::new();

        let spawner = pool.spawner();
        let latch = CountDownLatch::new(100);

        for _ in 0..200 {
            let latch1 = latch.clone();
            spawner
                .spawn(async move { latch1.count_down().await })
                .unwrap();
        }

        pool.run();
    }

    #[test]
    fn countdownlatch_post_wait_test() {
        let mut pool = LocalPool::new();

        let spawner = pool.spawner();
        let latch = CountDownLatch::new(100);

        for _ in 0..200 {
            let latch1 = latch.clone();
            spawner
                .spawn(async move { latch1.count_down().await })
                .unwrap();
        }

        pool.run();

        for _ in 0..100 {
            let latch1 = latch.clone();
            spawner.spawn(async move { latch1.wait().await }).unwrap();
        }

        pool.run();
    }

    #[test]
    fn countdownlatch_count_test() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let mut pool = LocalPool::new();
        let pre_counter = Arc::new(AtomicUsize::new(0));
        let post_counter = Arc::new(AtomicUsize::new(0));

        let spawner = pool.spawner();
        let latch = CountDownLatch::new(1);

        let latch1 = latch.clone();
        let pre_counter1 = pre_counter.clone();
        let post_counter1 = post_counter.clone();
        spawner
            .spawn(async move {
                pre_counter1.store(latch1.count().await, Ordering::Relaxed);
                latch1.count_down().await;
                post_counter1.store(latch1.count().await, Ordering::Relaxed);
            })
            .unwrap();

        pool.run();

        assert_eq!(1, pre_counter.load(Ordering::Relaxed));
        assert_eq!(0, post_counter.load(Ordering::Relaxed));
    }

    #[test]
    fn wait_with_timeout_test() {
        use futures_timer::Delay;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Arc;

        let mut pool = LocalPool::new();
        let counter = Arc::new(AtomicUsize::new(1));
        let no_timeout = Arc::new(AtomicBool::new(true));

        let spawner = pool.spawner();
        let latch = CountDownLatch::new(1);

        let latch1 = latch.clone();
        spawner
            .spawn(async move {
                Delay::new(Duration::from_secs(3)).await;
                latch1.count_down().await;
            })
            .unwrap();

        let counter1 = counter.clone();
        let no_timeout1 = no_timeout.clone();
        spawner
            .spawn(async move {
                let result = latch.wait_for(Duration::from_secs(1)).await;
                counter1.store(latch.count().await, Ordering::Relaxed);
                no_timeout1.store(result, Ordering::Relaxed);
            })
            .unwrap();

        pool.run();

        assert_eq!(1, counter.load(Ordering::Relaxed));
        assert_eq!(false, no_timeout.load(Ordering::Relaxed));
    }

    #[test]
    fn stress_test() {
        let mut pool = LocalPool::new();

        let n = 10_000;
        let latch = CountDownLatch::new(n);

        let spawner = pool.spawner();

        for _ in 0..(2 * n) {
            let latch1 = latch.clone();
            spawner.spawn(async move {
                latch1.wait().await;
            }).unwrap();
        }

        for _ in 0..n {
            let latch2 = latch.clone();
            spawner.spawn(async move {
                latch2.count_down().await;
            }).unwrap();
        }

        for _ in 0..(2 * n) {
            let latch3 = latch.clone();
            spawner.spawn(async move {
                latch3.wait().await;
            }).unwrap();
        }

        pool.run();
    }
}
