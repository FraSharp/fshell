// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Cache publication is an event, not a reason to poll the editor faster.

use std::sync::LazyLock;
use tokio::sync::watch;

static CHANGES: LazyLock<watch::Sender<()>> = LazyLock::new(|| watch::channel(()).0);

pub(crate) fn subscribe() -> watch::Receiver<()> {
    CHANGES.subscribe()
}

/// Call after committing and unlocking the cache. Watch versions coalesce
/// bursts while retaining an update that arrives during an active compute.
pub(crate) fn publish() {
    CHANGES.send_replace(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publication_is_retained_while_the_consumer_is_busy() {
        let mut changes = subscribe();
        publish();
        publish();
        tokio::time::timeout(std::time::Duration::from_secs(1), changes.changed())
            .await
            .expect("publication deadline")
            .expect("publication channel");
    }
}
