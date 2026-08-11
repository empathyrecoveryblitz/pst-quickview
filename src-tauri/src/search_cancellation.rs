use rusqlite::{Connection, ErrorCode, InterruptHandle};
use std::{
    collections::HashMap,
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

pub(crate) const SEARCH_CANCELLED_CODE: &str = "SEARCH_CANCELLED";
const MAX_TRACKED_WINDOWS: usize = 16;
const MAX_ACTIVE_OPERATIONS_PER_WINDOW: usize = 64;
const MAX_OPERATION_ID_SCALARS: usize = 96;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchOperationCategory {
    MessagePage,
    MessageCount,
    ConversationPage,
    ConversationCount,
    ExpandedConversation,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SearchCancellationError {
    Cancelled,
    InvalidOperationId,
    DuplicateOperation,
    CapacityExceeded,
    RegistryUnavailable,
}

impl fmt::Display for SearchCancellationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Cancelled => "Search cancelled.",
            Self::InvalidOperationId => "Invalid search operation identifier.",
            Self::DuplicateOperation => "Search operation identifier is already active.",
            Self::CapacityExceeded => "Too many search operations are active.",
            Self::RegistryUnavailable => "Search cancellation state is unavailable.",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for SearchCancellationError {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SearchCancellationOutcome {
    pub(crate) operations: usize,
    pub(crate) handles: usize,
}

#[derive(Default)]
pub(crate) struct SearchCancellationRegistry {
    inner: Mutex<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    windows: HashMap<String, WindowSearchState>,
}

struct WindowSearchState {
    generation: u64,
    generation_cancelled: bool,
    operations: HashMap<String, OperationEntry>,
}

impl WindowSearchState {
    fn new(generation: u64, generation_cancelled: bool) -> Self {
        Self {
            generation,
            generation_cancelled,
            operations: HashMap::new(),
        }
    }
}

struct OperationEntry {
    _category: SearchOperationCategory,
    cancelled: Arc<AtomicBool>,
    handles: Vec<InterruptHandle>,
}

pub(crate) struct SearchOperationGuard {
    registry: Arc<SearchCancellationRegistry>,
    window_label: String,
    generation: u64,
    operation_id: String,
    cancelled: Arc<AtomicBool>,
}

impl SearchCancellationRegistry {
    pub(crate) fn begin_operation(
        self: &Arc<Self>,
        window_label: &str,
        generation: u64,
        operation_id: &str,
        category: SearchOperationCategory,
    ) -> Result<SearchOperationGuard, SearchCancellationError> {
        validate_operation_id(operation_id)?;

        let cancelled = Arc::new(AtomicBool::new(false));
        let mut handles_to_interrupt = Vec::new();
        {
            let mut registry = self
                .inner
                .lock()
                .map_err(|_| SearchCancellationError::RegistryUnavailable)?;
            if !registry.windows.contains_key(window_label)
                && registry.windows.len() >= MAX_TRACKED_WINDOWS
            {
                return Err(SearchCancellationError::CapacityExceeded);
            }
            let state = registry
                .windows
                .entry(window_label.to_string())
                .or_insert_with(|| WindowSearchState::new(generation, false));

            if generation < state.generation
                || (generation == state.generation && state.generation_cancelled)
            {
                return Err(SearchCancellationError::Cancelled);
            }

            if generation > state.generation {
                handles_to_interrupt = cancel_all_operations(state);
                state.generation = generation;
                state.generation_cancelled = false;
            }

            if state.operations.len() >= MAX_ACTIVE_OPERATIONS_PER_WINDOW {
                return Err(SearchCancellationError::CapacityExceeded);
            }
            if state.operations.contains_key(operation_id) {
                return Err(SearchCancellationError::DuplicateOperation);
            }
            state.operations.insert(
                operation_id.to_string(),
                OperationEntry {
                    _category: category,
                    cancelled: Arc::clone(&cancelled),
                    handles: Vec::new(),
                },
            );
        }
        interrupt_all(handles_to_interrupt);

        Ok(SearchOperationGuard {
            registry: Arc::clone(self),
            window_label: window_label.to_string(),
            generation,
            operation_id: operation_id.to_string(),
            cancelled,
        })
    }

    pub(crate) fn cancel_generation(
        &self,
        window_label: &str,
        generation: u64,
    ) -> Result<SearchCancellationOutcome, SearchCancellationError> {
        let (operations, handles) = {
            let mut registry = self
                .inner
                .lock()
                .map_err(|_| SearchCancellationError::RegistryUnavailable)?;
            if !registry.windows.contains_key(window_label)
                && registry.windows.len() >= MAX_TRACKED_WINDOWS
            {
                return Err(SearchCancellationError::CapacityExceeded);
            }
            let state = registry
                .windows
                .entry(window_label.to_string())
                .or_insert_with(|| WindowSearchState::new(generation, true));

            if generation < state.generation {
                return Ok(SearchCancellationOutcome::default());
            }
            if generation > state.generation {
                let operations = state.operations.len();
                let handles = cancel_all_operations(state);
                state.generation = generation;
                state.generation_cancelled = true;
                (operations, handles)
            } else {
                let operations = state.operations.len();
                let handles = cancel_all_operations(state);
                state.generation_cancelled = true;
                (operations, handles)
            }
        };
        let handle_count = handles.len();
        interrupt_all(handles);
        Ok(SearchCancellationOutcome {
            operations,
            handles: handle_count,
        })
    }

    pub(crate) fn cancel_operation(
        &self,
        window_label: &str,
        generation: u64,
        operation_id: &str,
    ) -> Result<SearchCancellationOutcome, SearchCancellationError> {
        validate_operation_id(operation_id)?;
        let handles = {
            let mut registry = self
                .inner
                .lock()
                .map_err(|_| SearchCancellationError::RegistryUnavailable)?;
            let Some(state) = registry.windows.get_mut(window_label) else {
                return Ok(SearchCancellationOutcome::default());
            };
            if generation != state.generation || state.generation_cancelled {
                return Ok(SearchCancellationOutcome::default());
            }
            let Some(operation) = state.operations.remove(operation_id) else {
                return Ok(SearchCancellationOutcome::default());
            };
            operation.cancelled.store(true, Ordering::Release);
            operation.handles
        };
        let handle_count = handles.len();
        interrupt_all(handles);
        Ok(SearchCancellationOutcome {
            operations: 1,
            handles: handle_count,
        })
    }

    fn register_connection(
        &self,
        guard: &SearchOperationGuard,
        connection: &Connection,
    ) -> Result<(), SearchCancellationError> {
        let mut handle = Some(connection.get_interrupt_handle());
        let registered = {
            let mut registry = self
                .inner
                .lock()
                .map_err(|_| SearchCancellationError::RegistryUnavailable)?;
            let Some(state) = registry.windows.get_mut(&guard.window_label) else {
                return Err(SearchCancellationError::Cancelled);
            };
            if state.generation != guard.generation || state.generation_cancelled {
                false
            } else if let Some(operation) = state.operations.get_mut(&guard.operation_id) {
                if Arc::ptr_eq(&operation.cancelled, &guard.cancelled)
                    && !operation.cancelled.load(Ordering::Acquire)
                {
                    operation
                        .handles
                        .push(handle.take().expect("interrupt handle was present"));
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };

        if registered {
            Ok(())
        } else {
            if let Some(handle) = handle {
                handle.interrupt();
            }
            Err(SearchCancellationError::Cancelled)
        }
    }

    fn complete_operation(&self, guard: &SearchOperationGuard) {
        let Ok(mut registry) = self.inner.lock() else {
            return;
        };
        let Some(state) = registry.windows.get_mut(&guard.window_label) else {
            return;
        };
        if state.generation != guard.generation {
            return;
        }
        let should_remove = state
            .operations
            .get(&guard.operation_id)
            .is_some_and(|operation| Arc::ptr_eq(&operation.cancelled, &guard.cancelled));
        if should_remove {
            state.operations.remove(&guard.operation_id);
        }
    }

    #[cfg(test)]
    fn active_operation_count(&self, window_label: &str) -> usize {
        self.inner
            .lock()
            .ok()
            .and_then(|registry| {
                registry
                    .windows
                    .get(window_label)
                    .map(|state| state.operations.len())
            })
            .unwrap_or(0)
    }

    #[cfg(test)]
    fn operation_category(
        &self,
        window_label: &str,
        operation_id: &str,
    ) -> Option<SearchOperationCategory> {
        self.inner.lock().ok().and_then(|registry| {
            registry
                .windows
                .get(window_label)
                .and_then(|state| state.operations.get(operation_id))
                .map(|operation| operation._category)
        })
    }
}

impl SearchOperationGuard {
    pub(crate) fn register_connection(
        &self,
        connection: &Connection,
    ) -> Result<(), SearchCancellationError> {
        self.registry.register_connection(self, connection)
    }

    pub(crate) fn check_cancelled(&self) -> Result<(), SearchCancellationError> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(SearchCancellationError::Cancelled)
        } else {
            Ok(())
        }
    }

    #[cfg(test)]
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Drop for SearchOperationGuard {
    fn drop(&mut self) {
        self.registry.complete_operation(self);
    }
}

pub(crate) fn is_sqlite_interrupt(error: &rusqlite::Error) -> bool {
    error.sqlite_error_code() == Some(ErrorCode::OperationInterrupted)
}

fn validate_operation_id(operation_id: &str) -> Result<(), SearchCancellationError> {
    let length = operation_id.chars().count();
    if length == 0
        || length > MAX_OPERATION_ID_SCALARS
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(SearchCancellationError::InvalidOperationId);
    }
    Ok(())
}

fn cancel_all_operations(state: &mut WindowSearchState) -> Vec<InterruptHandle> {
    let operations = std::mem::take(&mut state.operations);
    let mut handles = Vec::new();
    for (_, operation) in operations {
        operation.cancelled.store(true, Ordering::Release);
        handles.extend(operation.handles);
    }
    handles
}

fn interrupt_all(handles: Vec<InterruptHandle>) {
    for handle in handles {
        handle.interrupt();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::ffi;
    use std::{
        ffi::c_void,
        os::raw::c_int,
        ptr,
        sync::{Condvar, Mutex as StdMutex},
        thread,
    };

    const WINDOW: &str = "main";

    fn registry() -> Arc<SearchCancellationRegistry> {
        Arc::new(SearchCancellationRegistry::default())
    }

    fn begin(
        registry: &Arc<SearchCancellationRegistry>,
        generation: u64,
        operation_id: &str,
        category: SearchOperationCategory,
    ) -> SearchOperationGuard {
        registry
            .begin_operation(WINDOW, generation, operation_id, category)
            .expect("operation should start")
    }

    #[test]
    fn registers_and_completes_one_operation() {
        let registry = registry();
        {
            let operation = begin(
                &registry,
                1,
                "messages-1",
                SearchOperationCategory::MessagePage,
            );
            assert_eq!(registry.active_operation_count(WINDOW), 1);
            assert_eq!(
                registry.operation_category(WINDOW, "messages-1"),
                Some(SearchOperationCategory::MessagePage)
            );
            drop(operation);
        }
        assert_eq!(registry.active_operation_count(WINDOW), 0);
    }

    #[test]
    fn newer_generation_cancels_older_and_obsolete_generation_is_rejected() {
        let registry = registry();
        let old = begin(
            &registry,
            3,
            "messages-1",
            SearchOperationCategory::MessagePage,
        );
        let current = begin(
            &registry,
            4,
            "conversations-1",
            SearchOperationCategory::ConversationPage,
        );
        assert!(old.is_cancelled());
        assert!(!current.is_cancelled());
        assert_eq!(registry.active_operation_count(WINDOW), 1);
        assert!(matches!(
            registry.begin_operation(
                WINDOW,
                3,
                "messages-2",
                SearchOperationCategory::MessagePage
            ),
            Err(SearchCancellationError::Cancelled)
        ));
    }

    #[test]
    fn same_generation_supports_distinct_operations_and_exact_cancel_isolated() {
        let registry = registry();
        let messages = begin(
            &registry,
            2,
            "messages-1",
            SearchOperationCategory::MessagePage,
        );
        let expanded = begin(
            &registry,
            2,
            "expanded-conversation-1",
            SearchOperationCategory::ExpandedConversation,
        );
        let outcome = registry
            .cancel_operation(WINDOW, 2, "expanded-conversation-1")
            .unwrap();
        assert_eq!(outcome.operations, 1);
        assert!(expanded.is_cancelled());
        assert!(!messages.is_cancelled());
        assert_eq!(registry.active_operation_count(WINDOW), 1);
    }

    #[test]
    fn page_and_count_complete_independently_and_new_generation_cancels_both() {
        let registry = registry();
        let page = begin(
            &registry,
            6,
            "message-page-1",
            SearchOperationCategory::MessagePage,
        );
        let count = begin(
            &registry,
            6,
            "message-count-1",
            SearchOperationCategory::MessageCount,
        );
        assert_eq!(registry.active_operation_count(WINDOW), 2);
        drop(page);
        assert_eq!(registry.active_operation_count(WINDOW), 1);
        assert_eq!(
            registry.operation_category(WINDOW, "message-count-1"),
            Some(SearchOperationCategory::MessageCount)
        );

        let next = begin(
            &registry,
            7,
            "conversation-page-1",
            SearchOperationCategory::ConversationPage,
        );
        assert!(count.is_cancelled());
        assert!(!next.is_cancelled());
        assert_eq!(registry.active_operation_count(WINDOW), 1);
        drop(next);
        assert_eq!(registry.active_operation_count(WINDOW), 0);
    }

    #[test]
    fn generation_cancel_interrupts_every_operation_and_is_idempotent() {
        let registry = registry();
        let first = begin(
            &registry,
            5,
            "messages-1",
            SearchOperationCategory::MessagePage,
        );
        let second = begin(
            &registry,
            5,
            "conversations-1",
            SearchOperationCategory::ConversationPage,
        );
        let outcome = registry.cancel_generation(WINDOW, 5).unwrap();
        assert_eq!(outcome.operations, 2);
        assert!(first.is_cancelled());
        assert!(second.is_cancelled());
        assert_eq!(registry.active_operation_count(WINDOW), 0);
        assert_eq!(
            registry.cancel_generation(WINDOW, 5).unwrap(),
            SearchCancellationOutcome::default()
        );
        assert!(matches!(
            registry.begin_operation(
                WINDOW,
                5,
                "messages-2",
                SearchOperationCategory::MessagePage
            ),
            Err(SearchCancellationError::Cancelled)
        ));
    }

    #[test]
    fn stale_cancel_cannot_affect_newer_generation() {
        let registry = registry();
        let current = begin(
            &registry,
            8,
            "messages-1",
            SearchOperationCategory::MessagePage,
        );
        assert_eq!(
            registry.cancel_generation(WINDOW, 7).unwrap(),
            SearchCancellationOutcome::default()
        );
        assert_eq!(
            registry.cancel_operation(WINDOW, 7, "messages-1").unwrap(),
            SearchCancellationOutcome::default()
        );
        assert!(!current.is_cancelled());
    }

    #[test]
    fn one_operation_tracks_every_workspace_connection_handle() {
        let registry = registry();
        let operation = begin(
            &registry,
            1,
            "messages-1",
            SearchOperationCategory::MessagePage,
        );
        let first = Connection::open_in_memory().unwrap();
        let second = Connection::open_in_memory().unwrap();
        operation.register_connection(&first).unwrap();
        operation.register_connection(&second).unwrap();

        let outcome = registry.cancel_generation(WINDOW, 1).unwrap();
        assert_eq!(outcome.operations, 1);
        assert_eq!(outcome.handles, 2);
        assert!(operation.is_cancelled());
    }

    #[test]
    fn completed_operations_do_not_grow_the_registry() {
        let registry = registry();
        for index in 0..512 {
            let operation = begin(
                &registry,
                1,
                &format!("messages-{index}"),
                SearchOperationCategory::MessagePage,
            );
            drop(operation);
        }
        assert_eq!(registry.active_operation_count(WINDOW), 0);
    }

    #[test]
    fn operation_ids_are_opaque_and_strictly_bounded() {
        let registry = registry();
        for invalid in ["", "contains query", "message:1", "message/path"] {
            assert!(matches!(
                registry.begin_operation(WINDOW, 1, invalid, SearchOperationCategory::MessagePage),
                Err(SearchCancellationError::InvalidOperationId)
            ));
        }
        let oversized = "a".repeat(MAX_OPERATION_ID_SCALARS + 1);
        assert!(matches!(
            registry.begin_operation(WINDOW, 1, &oversized, SearchOperationCategory::MessagePage),
            Err(SearchCancellationError::InvalidOperationId)
        ));
    }

    struct ProgressGate {
        callback_count: StdMutex<usize>,
        condition: Condvar,
    }

    const REQUIRED_PROGRESS_CALLBACKS: usize = 4_096;

    unsafe extern "C" fn progress_gate_callback(context: *mut c_void) -> c_int {
        // The raw Arc remains alive until the handler is removed after the query.
        let gate = unsafe { &*(context.cast::<ProgressGate>()) };
        let mut callback_count = gate.callback_count.lock().expect("progress gate lock");
        *callback_count += 1;
        if *callback_count == REQUIRED_PROGRESS_CALLBACKS {
            gate.condition.notify_all();
        }
        0
    }

    #[test]
    fn sqlite_query_is_physically_interrupted_and_connection_remains_usable() {
        let registry = registry();
        let operation = begin(
            &registry,
            11,
            "message-count-1",
            SearchOperationCategory::MessageCount,
        );
        let connection = Connection::open_in_memory().unwrap();
        operation.register_connection(&connection).unwrap();

        let gate = Arc::new(ProgressGate {
            callback_count: StdMutex::new(0),
            condition: Condvar::new(),
        });
        let raw_gate = Arc::into_raw(Arc::clone(&gate)).cast_mut().cast::<c_void>();
        let raw_gate_address = raw_gate as usize;
        unsafe {
            ffi::sqlite3_progress_handler(
                connection.handle(),
                1,
                Some(progress_gate_callback),
                raw_gate,
            );
        }

        let worker = thread::spawn(move || {
            let raw_gate = raw_gate_address as *mut c_void;
            let error = connection
                .query_row(
                    "WITH RECURSIVE numbers(value) AS (
                         VALUES(0) UNION ALL SELECT value + 1 FROM numbers WHERE value < 1000000000
                     ) SELECT sum(value) FROM numbers",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect_err("query should be interrupted");
            unsafe {
                ffi::sqlite3_progress_handler(connection.handle(), 0, None, ptr::null_mut());
                drop(Arc::from_raw(raw_gate.cast::<ProgressGate>()));
            }
            let still_usable = connection
                .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap();
            (error, still_usable, operation)
        });

        let mut callback_count = gate.callback_count.lock().unwrap();
        while *callback_count < REQUIRED_PROGRESS_CALLBACKS {
            callback_count = gate.condition.wait(callback_count).unwrap();
        }
        drop(callback_count);
        let outcome = registry.cancel_generation(WINDOW, 11).unwrap();
        assert_eq!(outcome.handles, 1);

        let (error, still_usable, operation) = worker.join().unwrap();
        assert!(is_sqlite_interrupt(&error));
        assert_eq!(still_usable, 1);
        assert!(operation.is_cancelled());
    }

    #[test]
    fn real_sqlite_errors_are_not_classified_as_cancellation() {
        assert!(!is_sqlite_interrupt(&rusqlite::Error::QueryReturnedNoRows));
        let connection = Connection::open_in_memory().unwrap();
        let error = connection
            .execute("SELECT * FROM missing_table", [])
            .expect_err("missing table should fail");
        assert!(!is_sqlite_interrupt(&error));
    }
}
