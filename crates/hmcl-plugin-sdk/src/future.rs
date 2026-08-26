use crate::{Error, ErrorCode, Value};
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::task::Poll;

const MAX_PENDING_CALLBACKS: usize = 1024;

pub(crate) struct CallbackRegistry {
    state: Mutex<RegistryState>,
}

struct RegistryState {
    closed: bool,
    next_id: u64,
    entries: HashMap<u64, Arc<Entry>>,
}

struct Entry {
    state: Mutex<EntryState>,
    changed: Condvar,
}

#[derive(Clone)]
enum EntryState {
    Pending,
    Complete(Result<Value, Error>),
    Cancelled,
}

impl CallbackRegistry {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState {
                closed: false,
                next_id: 1,
                entries: HashMap::new(),
            }),
        }
    }

    pub(crate) fn pair(self: &Arc<Self>) -> Result<(Callback, PluginFuture), Error> {
        let mut registry = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if registry.closed || registry.entries.len() >= MAX_PENDING_CALLBACKS {
            return Err(Error::new(ErrorCode::Unavailable));
        }
        let Some(id) = vacant_id(&registry) else {
            return Err(Error::new(ErrorCode::Unavailable));
        };
        registry.next_id = successor(id);
        let entry = Arc::new(Entry {
            state: Mutex::new(EntryState::Pending),
            changed: Condvar::new(),
        });
        registry.entries.insert(id, Arc::clone(&entry));
        Ok((
            Callback {
                registry: Arc::downgrade(self),
                id,
                entry: Arc::clone(&entry),
            },
            PluginFuture {
                registry: Arc::clone(self),
                id,
                entry,
            },
        ))
    }

    pub(crate) fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entries
            .len()
    }

    pub(crate) fn close(&self) {
        let entries = {
            let mut registry = self.state.lock().unwrap_or_else(|error| error.into_inner());
            registry.closed = true;
            registry
                .entries
                .drain()
                .map(|(_, entry)| entry)
                .collect::<Vec<_>>()
        };
        for entry in entries {
            let mut state = entry
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if matches!(*state, EntryState::Pending) {
                *state = EntryState::Cancelled;
                entry.changed.notify_all();
            }
        }
    }

    fn remove(&self, id: u64, entry: &Arc<Entry>) {
        let mut registry = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if registry
            .entries
            .get(&id)
            .is_some_and(|registered| Arc::ptr_eq(registered, entry))
        {
            registry.entries.remove(&id);
        }
    }
}

fn vacant_id(registry: &RegistryState) -> Option<u64> {
    let mut candidate = if registry.next_id == 0 {
        1
    } else {
        registry.next_id
    };
    // Among `live_count + 1` distinct candidates, at least one is vacant while below capacity.
    for _ in 0..=registry.entries.len() {
        if !registry.entries.contains_key(&candidate) {
            return Some(candidate);
        }
        candidate = successor(candidate);
    }
    None
}

fn successor(id: u64) -> u64 {
    id.checked_add(1).unwrap_or(1)
}

/// A context-scoped, exactly-once completion capability.
///
/// Transport of callback IDs to a runtime host is intentionally outside Bridge ABI v1. This
/// capability provides the SDK-local completion and cancellation semantics used by later host
/// adapters.
pub struct Callback {
    registry: Weak<CallbackRegistry>,
    id: u64,
    entry: Arc<Entry>,
}

impl Callback {
    /// Completes the paired future exactly once.
    pub fn complete(&self, result: Result<Value, Error>) -> Result<(), Error> {
        let mut state = self
            .entry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match &*state {
            EntryState::Pending => {
                let Some(registry) = self.registry.upgrade() else {
                    *state = EntryState::Cancelled;
                    self.entry.changed.notify_all();
                    return Err(Error::new(ErrorCode::Cancelled));
                };
                let mut registry_state = registry
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if registry_state.closed
                    || !registry_state
                        .entries
                        .get(&self.id)
                        .is_some_and(|registered| Arc::ptr_eq(registered, &self.entry))
                {
                    *state = EntryState::Cancelled;
                    self.entry.changed.notify_all();
                    return Err(Error::new(ErrorCode::Cancelled));
                }
                *state = EntryState::Complete(result);
                registry_state.entries.remove(&self.id);
                self.entry.changed.notify_all();
                drop(registry_state);
                drop(state);
                Ok(())
            }
            EntryState::Complete(_) => Err(Error::new(ErrorCode::CallbackFailed)),
            EntryState::Cancelled => Err(Error::new(ErrorCode::Cancelled)),
        }
    }
}

/// A blocking or pollable result paired with one [`Callback`].
pub struct PluginFuture {
    registry: Arc<CallbackRegistry>,
    id: u64,
    entry: Arc<Entry>,
}

impl PluginFuture {
    /// Returns the current terminal result without blocking.
    pub fn poll(&self) -> Poll<Result<Value, Error>> {
        match &*self
            .entry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
        {
            EntryState::Pending => Poll::Pending,
            EntryState::Complete(result) => Poll::Ready(result.clone()),
            EntryState::Cancelled => Poll::Ready(Err(Error::new(ErrorCode::Cancelled))),
        }
    }

    /// Blocks until completion or cancellation and returns the terminal result.
    pub fn wait(&self) -> Result<Value, Error> {
        let mut state = self
            .entry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while matches!(*state, EntryState::Pending) {
            state = self
                .entry
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        match &*state {
            EntryState::Complete(result) => result.clone(),
            EntryState::Cancelled => Err(Error::new(ErrorCode::Cancelled)),
            EntryState::Pending => unreachable!("wait loop exits only on a terminal state"),
        }
    }

    /// Cancels a pending future, returning whether this call won the terminal transition.
    pub fn cancel(&self) -> bool {
        let mut state = self
            .entry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !matches!(*state, EntryState::Pending) {
            return false;
        }
        *state = EntryState::Cancelled;
        self.entry.changed.notify_all();
        drop(state);
        self.registry.remove(self.id, &self.entry);
        true
    }
}

impl Drop for PluginFuture {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_id_rollover_skips_live_low_ids_without_overwriting_entries() {
        let registry = Arc::new(CallbackRegistry::new());
        let (first_callback, first_future) = registry.pair().expect("first pair");
        let (second_callback, second_future) = registry.pair().expect("second pair");
        assert_eq!((first_callback.id, second_callback.id), (1, 2));
        registry
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .next_id = u64::MAX;

        let (maximum_callback, maximum_future) = registry.pair().expect("maximum pair");
        let (wrapped_callback, wrapped_future) = registry.pair().expect("wrapped pair");
        assert_eq!(maximum_callback.id, u64::MAX);
        assert_eq!(wrapped_callback.id, 3);
        assert_eq!(registry.len(), 4);

        drop((first_future, second_future, maximum_future, wrapped_future));
        assert_eq!(registry.len(), 0);
    }
}
