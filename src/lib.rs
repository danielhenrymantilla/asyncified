mod channel;
mod oneshot;

type Func<T> = Box<dyn FnOnce(&mut T) + Send + 'static>;

/// This struct is designed to take a value whose methods are
/// blocking and potentially slow, move it onto its own thread,
/// and provide an async interface to operate on it.
#[derive(Clone, Debug)]
pub struct Asyncified<T> {
    tx: channel::Sender<Func<T>>
}

impl <T: Send + 'static> Asyncified<T> {
    /// This function takes the provided value and moves it into a
    /// newly spawned thread. Use the [`Asyncified::call()`] method to
    /// operate on this value in that thread.
    pub fn new(mut val: T) -> Self {
        let (tx, mut rx) = channel::new::<Func<T>>(10);

        // As long as there are senders, we'll
        // receive and run functions in the separate thread.
        std::thread::spawn(move || {
            while let Some(f) = rx.recv() {
                f(&mut val)
            }
        });

        Self { tx }
    }

    /// Execute the provided function on the thread that the asyncified
    /// value was moved onto, handing back the result when done. This will
    /// not block while the function is running.
    pub async fn call<R: Send + 'static, F: (FnOnce(&mut T) -> R) + Send + 'static>(&self, f: F) -> R {
        let (tx, rx) = oneshot::new::<R>();

        // Ignore any error, since we expect the thread to last until this
        // struct is dropped, and so sending should never fail.
        let _ = self.tx.send(Box::new(move |item| {
            let res = f(item);
            // Send res back via sync->async oneshot.
            tx.send(res);
        })).await;

        rx.recv().await
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn basic_updating_works() {
        let a = Asyncified::new(0u64);

        for i in 1..100_000 {
            assert_eq!(a.call(|n| { *n += 1; *n }).await, i);
        }
    }

    #[tokio::test]
    async fn parallel_updating_works() {
        let a = Asyncified::new(0u64);

        // spawn 10 tasks which all increment the number
        let handles: Vec<_> = (0..10).map({
            let a = a.clone();
            move |_| {
                let a = a.clone();
                tokio::spawn(async move {
                    for _ in 0..10_000 {
                        a.call(|n| { *n += 1; *n }).await;
                    }
                })
            }
        }).collect();

        // wait for them all to finish
        for handle in handles {
            let _ = handle.await;
        }

        // the number should have been incremeted 100k times.
        assert_eq!(a.call(|n| *n).await, 100_000);
    }

    #[tokio::test]
    async fn is_non_blocking() {
        let a = Asyncified::new(());

        let start = std::time::Instant::now();

        // The function takes 10s to complete:
        let _fut = a.call(|_| {
            std::thread::sleep(std::time::Duration::from_secs(10));
        });

        // But we just get a future back which doesn't block:
        let d = std::time::Instant::now().duration_since(start).as_millis();
        assert!(d < 100);
    }
}