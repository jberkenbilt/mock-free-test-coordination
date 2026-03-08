//! The `scanner` module scans a directory waiting for files to
//! stabilize. When they have stabilized, a configured handler is
//! called. This code accompanies the blog post mentioned in the
//! top-level README.md. Comments throughout of the form "Note n: ..."
//! are expanded in the text of the post.
use async_trait::async_trait;
use regex::Regex;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc::WeakSender;
use tokio::sync::{Mutex, mpsc};

const DEFAULT_INTERVAL: Duration = Duration::from_secs(2);

// Note 1: we use traits and enums instead of multiple channels

// Rust 1.75 stabilized async functions in traits, but as of Rust
// 1.94, you still need async_trait for traits with async functions to
// be dyn-compatible.
#[async_trait]
pub trait Handler: Sync + Send {
    async fn handle(&self, name: &str, info: &Stat);
}

pub enum Event {
    Tick,
    Pattern(Option<String>),
}

#[cfg(test)]
struct TestEvent {
    state: HashMap<String, Stat>,
}

// Note 2: the scanner struct contains all private fields. An instance
// is created by instantiating a ScannerBuilder and call its
// fluent-style methods. When done, use the `build()` method to create
// a Scanner.

#[derive(Default)]
pub struct ScannerBuilder {
    handler: Option<Box<dyn Handler>>,
    interval: Option<Duration>,
    dir: Option<String>,
    no_tick: Option<()>,
    // Note 3: we use `#[cfg(test)]` throughout the code
    #[cfg(test)]
    test_tx: Option<mpsc::Sender<TestEvent>>,
}

pub struct Scanner {
    handler: Box<dyn Handler>,
    event_tx: mpsc::Sender<Event>,
    event_rx: Mutex<Option<mpsc::Receiver<Event>>>,
    interval: Duration,
    dir: String,
    tick: bool,
    #[cfg(test)]
    test_tx: Option<mpsc::Sender<TestEvent>>,
}

struct DefaultHandler;
#[async_trait]
impl Handler for DefaultHandler {
    async fn handle(&self, name: &str, _info: &Stat) {
        println!("name: {name}");
    }
}

// Note 4: the builder implementation
impl ScannerBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handler(mut self, handler: Box<dyn Handler>) -> Self {
        self.handler = Some(handler);
        self
    }

    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = Some(interval);
        self
    }

    pub fn dir(mut self, dir: &str) -> Self {
        self.dir = Some(dir.to_owned());
        self
    }

    #[cfg(test)]
    fn no_tick(mut self) -> Self {
        self.no_tick = Some(());
        self
    }

    #[cfg(test)]
    fn test_tx(mut self, tx: mpsc::Sender<TestEvent>) -> Self {
        self.test_tx = Some(tx);
        self
    }

    pub fn build(self) -> Scanner {
        let (event_tx, event_rx) = mpsc::channel(10);
        Scanner {
            handler: self.handler.unwrap_or(Box::new(DefaultHandler)),
            event_tx,
            event_rx: Mutex::new(Some(event_rx)),
            interval: self.interval.unwrap_or(DEFAULT_INTERVAL),
            dir: self.dir.unwrap_or(".".to_string()),
            tick: self.no_tick.is_none(),
            #[cfg(test)]
            test_tx: self.test_tx,
        }
    }
}

#[derive(Clone)]
pub struct Stat {
    size: u64,
    mod_time: SystemTime,
    called: bool,
}

// Note 5: send tick events to the event channel
fn tick_events(tx_weak: WeakSender<Event>, interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Discard the first tick, which happens immediately.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let Some(tx) = tx_weak.upgrade() else {
                return;
            };
            if tx.send(Event::Tick).await.is_err() {
                return;
            }
        }
    });
}

impl Scanner {
    pub async fn set_pattern(&self, pattern: Option<&str>) {
        // Ignore error; it means the channel has been closed
        _ = self
            .event_tx
            .send(Event::Pattern(pattern.map(str::to_owned)))
            .await;
    }

    // Note 6: test-only tick method
    #[cfg(test)]
    async fn tick(&self) {
        _ = self.event_tx.send(Event::Tick).await;
    }

    // Note 7: the run method
    pub async fn run(&self) {
        let Some(mut event_rx) = self.event_rx.lock().await.take() else {
            // The receiver has already been taken.
            return;
        };
        if self.tick {
            tick_events(self.event_tx.downgrade(), self.interval);
        }
        let mut pattern: Option<Regex> = None;
        let mut file_data: HashMap<String, Stat> = Default::default();
        while let Some(event) = event_rx.recv().await {
            match event {
                Event::Pattern(None) => return,
                Event::Pattern(Some(s)) => {
                    match Regex::new(&s) {
                        Ok(p) => pattern = Some(p),
                        Err(e) => {
                            log::error!("invalid regular expression: {e}");
                            pattern = None;
                        }
                    }
                    continue;
                }
                Event::Tick => {}
            }
            let entries = match fs::read_dir(&self.dir) {
                Err(e) => {
                    log::error!("unable to read {}: {e}", self.dir);
                    return;
                }
                Ok(rd) => rd,
            };
            let mut seen: HashSet<String> = Default::default();
            for entry in entries {
                let Ok(entry) = entry else {
                    continue;
                };
                let Ok(name) = String::from_utf8(entry.file_name().into_encoded_bytes()) else {
                    // Ignore files whose names are not valid UTF-8. They will never match the
                    // pattern.
                    continue;
                };
                let Some(pat) = &pattern else {
                    continue;
                };
                if !pat.is_match(&name) {
                    continue;
                }
                let metadata = match entry.metadata() {
                    Err(e) => {
                        log::warn!(
                            "unable to get metadata for {}/{}: {e}",
                            self.dir,
                            entry.file_name().display()
                        );
                        continue;
                    }
                    Ok(d) => d,
                };
                let stat = Stat {
                    size: metadata.len(),
                    // Real code might handle this error properly.
                    mod_time: metadata
                        .modified()
                        .expect("unable to get modification time"),
                    called: false,
                };
                seen.insert(name.clone());
                match file_data.entry(name.clone()) {
                    Entry::Occupied(mut e) => {
                        let old = e.get_mut();
                        if old.size == stat.size && old.mod_time == stat.mod_time {
                            if !old.called {
                                old.called = true;
                                self.handler.handle(&name, old).await;
                            }
                        } else {
                            e.insert(stat);
                        }
                    }
                    Entry::Vacant(e) => {
                        e.insert(stat);
                    }
                }
            }
            file_data.retain(|k, _| seen.contains(k));
            // Note 8: test support code
            #[cfg(test)]
            if let Some(test_tx) = &self.test_tx
                && test_tx
                    .send(TestEvent {
                        // We have to clone file_data because "fearless
                        // concurrency" in Rust prevents us from sharing a
                        // pointer. In real code, we could avoid this cost by
                        // using Arc<RwLock<T>>, or a cheaply clonable map type.
                        // For test/demo purposes, cloning the map is okay, and
                        // regardless, this clone only happens in test. The map
                        // is never cloned except behind the #[cfg(test)] gate.
                        // It would be possible to implement Clone for Stat only
                        // in test, but that is too much scope for this example.
                        state: file_data.clone(),
                    })
                    .await
                    .is_err()
            {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
