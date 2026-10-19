// SPDX-License-Identifier: GPL-3.0-only

//! Values sent to something slow, a program or a service, as a slider moves:
//! one at a time, and of those that came meanwhile only the newest. Sent all,
//! each by its own task, they queued up behind one another and the volume
//! went on changing well after the slider was let go, or ended on an older
//! value when they finished out of order.

use std::{collections::BTreeMap, sync::Mutex};

pub struct Latest<K, T> {
    /// The newest value for each key not yet sent, and whether a task is
    /// sending them.
    state: Mutex<(BTreeMap<K, T>, bool)>,
}

impl<K: Ord, T> Latest<K, T> {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new((BTreeMap::new(), false)),
        }
    }

    /// Sends `value` for `key` with `send`, after what is being sent now;
    /// returns at once when another task is sending, which will send it.
    pub async fn send<F: Future<Output = ()>>(&self, key: K, value: T, send: impl Fn(K, T) -> F) {
        {
            let mut state = self.state.lock().unwrap();
            state.0.insert(key, value);
            if state.1 {
                return;
            }
            state.1 = true;
        }
        loop {
            let next = {
                let mut state = self.state.lock().unwrap();
                let next = state.0.pop_first();
                state.1 = next.is_some();
                next
            };
            let Some((key, value)) = next else {
                return;
            };
            send(key, value).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn only_the_newest_after_the_one_sending() {
        static LATEST: Latest<(), u32> = Latest::new();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let wait = Arc::new(tokio::sync::Mutex::new(Some(wait)));
        let send = {
            let sent = sent.clone();
            move |(), value| {
                let (sent, wait) = (sent.clone(), wait.clone());
                async move {
                    // The first holds the line until released.
                    if let Some(wait) = wait.lock().await.take() {
                        _ = wait.await;
                    }
                    sent.lock().unwrap().push(value);
                }
            }
        };
        let first = tokio::spawn({
            let send = send.clone();
            async move { LATEST.send((), 1, send).await }
        });
        tokio::task::yield_now().await;
        for value in 2..=5 {
            LATEST.send((), value, send.clone()).await;
        }
        _ = release.send(());
        first.await.unwrap();
        assert_eq!(*sent.lock().unwrap(), vec![1, 5]);
    }
}
