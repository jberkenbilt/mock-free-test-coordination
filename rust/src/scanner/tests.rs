// This code accompanies the blog post mentioned in the top-level
// README.md. Comments throughout of the form "Note n: ..." are
// expanded in the text of the post.

use crate::scanner;
use crate::scanner::{Handler, ScannerBuilder, Stat};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;

struct TestHandler {
    tx: mpsc::Sender<String>,
}
#[async_trait]
impl Handler for TestHandler {
    async fn handle(&self, name: &str, _info: &Stat) {
        let _ = self.tx.send(name.to_string()).await;
    }
}

// Note 1: life-cycle test
#[tokio::test]
async fn test_lifecycle() {
    // Create a temporary directory for testing.
    let dir = tempfile::tempdir().unwrap();
    // Make a scanner that doesn't tick so we can control loop iterations.
    // Create a test channel so we can monitor private state, stored in
    // a local variable, after each iteration.
    let (test_tx, mut test_rx) = mpsc::channel(1);
    let (file_tx, mut file_rx) = mpsc::channel(1);
    let handler = Box::new(TestHandler { tx: file_tx });
    // Note 2: builder pattern
    let s = Arc::new(
        ScannerBuilder::new()
            .no_tick()
            .test_tx(test_tx)
            .dir(dir.path().to_str().unwrap())
            .handler(handler)
            .build(),
    );
    let s2 = s.clone();
    // Note 3: Run the `run` method
    let h = tokio::spawn(async move {
        s2.run().await;
    });
    // Note 4: Force one iteration of the loop. With async closures
    // stable since Rust 1.85, this is easy to do. Before that, we
    // would have had to create a separate function and pass things to
    // it.
    let mut iterate = async || {
        s.tick().await;
        test_rx.recv().await.unwrap().state
    };
    // Note 5: Run the full life cycle.
    let state = iterate().await;
    assert!(state.is_empty());
    // Set a pattern, create some files, and iterate. We don't expect
    // anything yet since no files will have stabilized in size and
    // modification time.
    s.set_pattern(Some(r".*\.a")).await;
    std::fs::write(dir.path().join("one.a"), b"potato").unwrap();
    std::fs::write(dir.path().join("one.b"), b"salad").unwrap();
    let state = iterate().await;
    assert!(file_rx.try_recv().is_err());
    assert!(state.contains_key("one.a"));
    // We don't expect one.b since it doesn't match the pattern.
    assert!(!state.contains_key("one.b"));
    iterate().await;
    // The file is stable now. It should be passed to the handler.
    assert_eq!(file_rx.recv().await.unwrap(), "one.a");
    // Iterate again. The handler should not be called again.
    iterate().await;
    assert!(file_rx.try_recv().is_err());

    // Change pattern and iterate. We should see the other file. Then
    // modify the file and iterate again. At the third iteration, the
    // file is stable, and the handler is called.
    s.set_pattern(Some(r".*\.b")).await;
    let state = iterate().await;
    assert!(!state.contains_key("one.a"));
    assert!(state.contains_key("one.b"));
    std::fs::write(dir.path().join("one.b"), b"salad!").unwrap();
    iterate().await;
    assert!(file_rx.try_recv().is_err());
    iterate().await;
    assert_eq!(file_rx.recv().await.unwrap(), "one.b");
    // We shouldn't get another handler call.
    iterate().await;
    assert!(file_rx.try_recv().is_err());

    // Send None as a pattern to cause the scanner to exit.
    s.set_pattern(None).await;
    h.await.unwrap();
}

// Note 6: test the real ticker
#[tokio::test]
async fn test_real_ticker() {
    // This test exercises using default values for everything except
    // the loop interval. It exercises that the default ticker works
    // by ensuring that two loop iterations take at least
    // approximately two intervals.
    let (test_tx, mut test_rx) = mpsc::channel(1);
    let s = Arc::new(
        ScannerBuilder::new()
            .test_tx(test_tx)
            .interval(Duration::from_millis(10))
            .build(),
    );
    let now = SystemTime::now();
    let s2 = s.clone();
    let h = tokio::spawn(async move {
        s2.run().await;
    });
    // We know the ticker is behaving like a ticker because we can see
    // multiple state updates without having to poke the loop.
    assert!(test_rx.recv().await.is_some());
    assert!(test_rx.recv().await.is_some());
    let elapsed = now.elapsed().unwrap();
    // This is a little bit fragile because of the upper bound check,
    // but we need some upper bound check to ensure that the
    // overridden interval is actually being seen. A test to make sure
    // it took less than the default interval does this and gives us a
    // great deal of headroom even in a congested CI environment. If
    // this fails, proceed with caution in loosening the upper bound
    // or use some other method to exercise the ticker.
    assert!(
        elapsed >= Duration::from_millis(20) && elapsed < scanner::DEFAULT_INTERVAL,
        "{elapsed:?}"
    );
    s.set_pattern(None).await;
    h.await.unwrap();
}
