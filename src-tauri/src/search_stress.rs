use super::*;
use rusqlite::ffi;
use std::{
    ffi::c_void,
    hint::black_box,
    os::raw::c_int,
    ptr,
    sync::{Condvar, Mutex as StdMutex},
};

static NEXT_STRESS_ROOT: AtomicUsize = AtomicUsize::new(0);
const STRESS_SEARCH_TERM: &str = "commonterm";
const SELECTIVE_SEARCH_TERM: &str = "rareterm";
const RANK_SEARCH_TERM: &str = "rankterm";

struct StressRoot {
    path: PathBuf,
}

impl StressRoot {
    fn new() -> Self {
        let sequence = NEXT_STRESS_ROOT.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "pst-quickview-search-stress-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("stress root should be created");
        Self { path }
    }
}

impl Drop for StressRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn synthetic_date(id: i64) -> String {
    let bucket = id / 5;
    let year = 2024 + bucket % 3;
    let month = 1 + bucket % 12;
    let day = 1 + (bucket / 12) % 28;
    let hour = (bucket / 336) % 24;
    let minute = (bucket / 8_064) % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:00+00:00")
}

fn synthetic_subject(id: i64) -> String {
    let mut subject = match id % 8 {
        0 => format!("Quarterly update {}", id % 97),
        1 => format!("Résumé review {}", id % 89),
        2 => format!("Café summary {}", id % 83),
        3 => format!("東京 project {}", id % 79),
        4 => format!("مشروع عربي {}", id % 73),
        5 => format!("Attachment review {}", id % 71),
        6 => format!("Duplicate subject {}", id % 17),
        _ => format!("Status note {}", id % 67),
    };
    if id % 100 == 0 {
        subject.push(' ');
        subject.push_str(RANK_SEARCH_TERM);
    }
    if id % 1_013 == 0 {
        subject.push_str(" subjectonly");
    }
    if id % 257 == 0 {
        subject.push_str(" 東京検索");
    }
    subject
}

fn synthetic_sender(id: i64) -> String {
    if id % 100 == 1 {
        return format!("{RANK_SEARCH_TERM}@example.test");
    }
    match id % 16 {
        0 => "álvaro@example.test".to_string(),
        1 => "renée@example.test".to_string(),
        2 => "東京@example.test".to_string(),
        3 => "مثال@example.test".to_string(),
        _ => format!("sender-{:02}@example.test", id % 32),
    }
}

fn synthetic_recipients(id: i64) -> String {
    if id % 100 == 2 {
        format!("{RANK_SEARCH_TERM}@example.test")
    } else if id % 29 == 0 {
        "recipient@example.test, recipient-two@example.test".to_string()
    } else {
        format!("recipient-{:02}@example.test", id % 24)
    }
}

fn synthetic_body(id: i64) -> String {
    let mut body = if id % 11 == 0 {
        format!(
            "{} Unicode résumé café 東京 本文 بحث عربي emoji 📎 commonterm",
            "long deterministic segment ".repeat(24)
        )
    } else {
        format!("Deterministic synthetic body {id} {STRESS_SEARCH_TERM}")
    };
    if id % 997 == 0 {
        body.push(' ');
        body.push_str(SELECTIVE_SEARCH_TERM);
    }
    if id % 100 == 3 {
        body.push(' ');
        body.push_str(RANK_SEARCH_TERM);
    }
    if id % 1_009 == 0 {
        body.push_str(" bodyonly");
    }
    if id % 263 == 0 {
        body.push_str(" بحثاختبار");
    }
    if id % 271 == 0 {
        body.push_str(" emojiadjacent");
    }
    body
}

fn synthetic_attachment_name(id: i64) -> String {
    if id % 100 == 4 {
        format!("{RANK_SEARCH_TERM}-attachment.pdf")
    } else if id % 50 == 0 {
        format!("deterministic-{:04}.pdf", id % 10_000)
    } else {
        String::new()
    }
}

fn create_synthetic_workspace(
    root: &Path,
    label: &str,
    workspace_id: &str,
    message_count: i64,
) -> ActiveWorkspace {
    let workspace = root.join(label);
    fs::create_dir_all(&workspace).expect("synthetic workspace should be created");
    let database_path = workspace.join("index.sqlite");
    let mut conn = Connection::open(&database_path).expect("synthetic database should open");
    initialize_schema(&conn).expect("synthetic schema should initialize");
    conn.execute_batch("PRAGMA synchronous = OFF; PRAGMA temp_store = MEMORY;")
        .expect("synthetic performance pragmas should apply");
    conn.execute_batch(
        "INSERT INTO folders (id, parent_id, path, name) VALUES
             (1, NULL, 'Inbox', 'Inbox'),
             (2, 1, 'Inbox/Projects', 'Projects'),
             (3, NULL, 'Archive', 'Archive');",
    )
    .expect("synthetic folders should insert");
    for (key, value) in [
        ("workspace_id", workspace_id.to_string()),
        ("import_status", "complete".to_string()),
        ("message_count_indexed", message_count.to_string()),
        (
            "conversation_schema_version",
            CONVERSATION_SCHEMA_VERSION.to_string(),
        ),
    ] {
        set_metadata_value(&conn, key, value).expect("synthetic metadata should insert");
    }

    let transaction = conn
        .transaction()
        .expect("synthetic insert transaction should start");
    {
        let mut message_statement = transaction
            .prepare_cached(
                "INSERT INTO messages (
                     id, folder_id, eml_path, subject, sender, recipients, date, body,
                     body_source, snippet, attachment_names, has_attachments,
                     normalized_subject, conversation_id, conversation_root_id,
                     thread_assignment_method
                 ) VALUES (
                     ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                     'text_plain', ?9, ?10, ?11, ?12, ?13, ?14, 'header'
                 )",
            )
            .expect("synthetic message statement should prepare");
        let mut attachment_statement = transaction
            .prepare_cached(
                "INSERT INTO attachments (
                     message_id, filename, sanitized_filename, content_type,
                     size_bytes, attachment_index, content_disposition, mime_part_path
                 ) VALUES (?1, ?2, ?2, 'application/pdf', ?3, 0, 'attachment', NULL)",
            )
            .expect("synthetic attachment statement should prepare");

        for id in 1..=message_count {
            let conversation_number = (id - 1) / 4 + 1;
            let conversation_root = (conversation_number - 1) * 4 + 1;
            let folder_id = match id % 3 {
                0 => 1,
                1 => 2,
                _ => 3,
            };
            let subject = synthetic_subject(id);
            let sender = synthetic_sender(id);
            let recipients = synthetic_recipients(id);
            let body = synthetic_body(id);
            let snippet = body.chars().take(220).collect::<String>();
            let attachment_name = synthetic_attachment_name(id);
            let has_attachment = !attachment_name.is_empty();
            message_statement
                .execute(params![
                    id,
                    folder_id,
                    format!("synthetic/{label}/{id}"),
                    subject,
                    sender,
                    recipients,
                    synthetic_date(id),
                    body,
                    snippet,
                    attachment_name,
                    i64::from(has_attachment),
                    format!("thread subject {}", conversation_number % 2_048),
                    format!("{workspace_id}-conversation-{conversation_number}"),
                    conversation_root,
                ])
                .expect("synthetic message should insert");
            if has_attachment {
                attachment_statement
                    .execute(params![
                        id,
                        synthetic_attachment_name(id),
                        1_024 + id % 8_192
                    ])
                    .expect("synthetic attachment should insert");
            }
        }
    }
    transaction
        .commit()
        .expect("synthetic insert transaction should commit");
    conn.execute_batch(
        "INSERT INTO messages_fts (
             rowid, subject, sender, recipients, body, attachment_names
         ) SELECT id, subject, sender, recipients, body, attachment_names FROM messages;
         PRAGMA optimize;",
    )
    .expect("synthetic FTS index should build");
    drop(conn);

    ActiveWorkspace {
        id: workspace_id.to_string(),
        path: workspace,
        pst_path: root.join(format!("{workspace_id}-synthetic-source")),
        fingerprint: format!("synthetic-fingerprint-{workspace_id}"),
        location_mode: WorkspaceLocationMode::AppSupport,
    }
}

fn percentile_ms(samples: &mut [Duration], percentile: f64) -> f64 {
    samples.sort_unstable();
    let index = ((samples.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(samples.len().saturating_sub(1));
    samples[index].as_secs_f64() * 1_000.0
}

fn measure(dataset_size: i64, metric: &str, iterations: usize, mut operation: impl FnMut()) {
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        operation();
        samples.push(started.elapsed());
    }
    let p50 = percentile_ms(&mut samples, 0.50);
    let p95 = percentile_ms(&mut samples, 0.95);
    println!(
        "SEARCH_STRESS dataset_rows={dataset_size} metric={metric} iterations={iterations} p50_ms={p50:.3} p95_ms={p95:.3}"
    );
}

fn collect_cursor_ids(
    workspace: &ActiveWorkspace,
    criteria: &MessageSearchCriteria,
    sort: &str,
    expected_count: usize,
) -> Vec<i64> {
    let conn = open_workspace_db_for_read(&workspace.path).expect("stress read should open");
    let codec = SearchCursorCodec::default();
    let mut cursor = None;
    let mut ids = Vec::with_capacity(expected_count);
    for _ in 0..expected_count.saturating_div(137) + 3 {
        let page = query_messages_cursor_page(
            &conn,
            &workspace.path,
            &workspace.id,
            None,
            false,
            criteria,
            Some(sort),
            Some(137),
            cursor.as_deref(),
            41,
            &codec,
            None,
        )
        .expect("stress cursor page should load");
        ids.extend(page.items.iter().map(|item| item.id));
        if !page.has_more {
            assert!(page.next_cursor.is_none());
            return ids;
        }
        cursor = page.next_cursor;
        assert!(cursor.is_some());
    }
    panic!("stress cursor pagination did not terminate");
}

fn verify_cursor_and_context_correctness(workspace: &ActiveWorkspace, message_count: usize) {
    let blank = MessageSearchCriteria::from_inputs(None, None).expect("blank criteria");
    let broad = MessageSearchCriteria::from_inputs(Some(STRESS_SEARCH_TERM.to_string()), None)
        .expect("broad criteria");
    for (sort, criteria) in [
        ("newest", &blank),
        ("oldest", &blank),
        ("sender_az", &blank),
        ("subject_az", &blank),
        ("relevance", &broad),
    ] {
        let ids = collect_cursor_ids(workspace, criteria, sort, message_count);
        assert_eq!(ids.len(), message_count, "{sort} omitted rows");
        assert_eq!(
            ids.iter().copied().collect::<HashSet<_>>().len(),
            message_count,
            "{sort} duplicated rows"
        );
    }

    let conn = open_workspace_db_for_read(&workspace.path).expect("stress read should open");
    let selective =
        MessageSearchCriteria::from_inputs(Some(SELECTIVE_SEARCH_TERM.to_string()), None)
            .expect("selective criteria");
    let selective_page = query_messages_page(
        &conn,
        None,
        false,
        &selective,
        Some("relevance"),
        Some(20),
        Some(0),
        None,
    )
    .expect("selective page should load");
    assert!(!selective_page.items.is_empty());
    assert!(selective_page.items.iter().all(|item| {
        item.search_match_context.as_ref().is_some_and(|context| {
            let snippet_utf16_len = context.snippet_text.encode_utf16().count();
            !context.matched_fields.is_empty()
                && context
                    .highlight_ranges
                    .iter()
                    .all(|range| range.start < range.end && range.end <= snippet_utf16_len)
        })
    }));

    let rank = MessageSearchCriteria::from_inputs(Some(RANK_SEARCH_TERM.to_string()), None)
        .expect("rank criteria");
    let ranked = query_messages_page(
        &conn,
        None,
        false,
        &rank,
        Some("relevance"),
        Some(1_000),
        Some(0),
        None,
    )
    .expect("ranked page should load");
    assert_eq!(ranked.items.first().map(|item| item.id % 100), Some(0));
    let best_subject = ranked
        .items
        .iter()
        .position(|item| item.id % 100 == 0)
        .expect("subject rank sample");
    let best_body = ranked
        .items
        .iter()
        .position(|item| item.id % 100 == 3)
        .expect("body rank sample");
    assert!(
        best_subject < best_body,
        "subject weighting must beat body-only match"
    );

    for term in ["résumé", "東京検索", "بحثاختبار"] {
        let unicode_criteria = MessageSearchCriteria::from_inputs(Some(term.to_string()), None)
            .expect("Unicode criteria");
        assert!(
            query_message_count(&conn, None, false, &unicode_criteria, None)
                .expect("Unicode count")
                > 0,
            "Unicode fixture term should remain searchable"
        );
    }
}

fn verify_multi_workspace_behavior(workspaces: &[ActiveWorkspace]) {
    let criteria = MessageSearchCriteria::from_inputs(Some(STRESS_SEARCH_TERM.to_string()), None)
        .expect("multi criteria");
    let registry = Arc::new(SearchCancellationRegistry::default());
    let first_operation = registry
        .begin_operation(
            "stress-multi",
            51,
            "message-page-1",
            SearchOperationCategory::MessagePage,
        )
        .expect("first multi operation");
    let first = query_multi_workspace_message_page(
        workspaces.to_vec(),
        &criteria,
        "newest",
        Some(250),
        Some(0),
        &first_operation,
    )
    .expect("first multi page");
    assert_eq!(first.pagination_mode, "offset");
    assert!(first.next_cursor.is_none());
    assert!(first.has_more);
    assert!(first.items.iter().all(|item| {
        item.workspace_id.is_some()
            && item.pst_display_name.is_some()
            && item.search_match_context.is_some()
    }));

    let second_operation = registry
        .begin_operation(
            "stress-multi",
            51,
            "message-page-2",
            SearchOperationCategory::MessagePage,
        )
        .expect("second multi operation");
    let second = query_multi_workspace_message_page(
        workspaces.to_vec(),
        &criteria,
        "newest",
        Some(250),
        Some(250),
        &second_operation,
    )
    .expect("second multi page");
    let first_keys = first
        .items
        .iter()
        .map(|item| (item.workspace_id.clone(), item.id))
        .collect::<HashSet<_>>();
    assert!(second
        .items
        .iter()
        .all(|item| !first_keys.contains(&(item.workspace_id.clone(), item.id))));

    let count_operation = registry
        .begin_operation(
            "stress-multi",
            51,
            "message-count-1",
            SearchOperationCategory::MessageCount,
        )
        .expect("multi count operation");
    let counts = count_multi_workspace_messages(workspaces.to_vec(), &criteria, &count_operation)
        .expect("multi count");
    assert_eq!(counts.per_workspace_counts.len(), workspaces.len());
    assert_eq!(
        counts.total_count,
        counts
            .per_workspace_counts
            .iter()
            .map(|count| count.count)
            .sum::<i64>()
    );
    let relevance_error =
        validate_message_sort_workspace_count(Some("relevance"), workspaces.len())
            .expect_err("cross-workspace relevance must remain rejected");
    assert!(relevance_error.to_string().contains("one PST workspace"));
    drop(count_operation);
    drop(second_operation);
    drop(first_operation);
}

fn verify_conversation_page_and_count(workspace: &ActiveWorkspace) {
    let criteria = MessageSearchCriteria::from_inputs(Some(STRESS_SEARCH_TERM.to_string()), None)
        .expect("conversation criteria");
    let scope = ConversationWorkspaceScope {
        workspace_id: workspace.id.clone(),
        folder_id: None,
        include_subfolders: false,
    };
    let registry = Arc::new(SearchCancellationRegistry::default());
    let page_operation = registry
        .begin_operation(
            "stress-conversations",
            61,
            "conversation-page-1",
            SearchOperationCategory::ConversationPage,
        )
        .expect("conversation page operation");
    let count_operation = registry
        .begin_operation(
            "stress-conversations",
            61,
            "conversation-count-1",
            SearchOperationCategory::ConversationCount,
        )
        .expect("conversation count operation");
    let page = query_conversation_page_for_scopes(
        vec![(scope.clone(), workspace.clone())],
        &criteria,
        "newest",
        Some(100),
        Some(0),
        &page_operation,
    )
    .expect("conversation page");
    assert_eq!(page.returned_count, 100);
    assert!(page.has_more);
    let counts = count_conversations_for_scopes(
        vec![(scope, workspace.clone())],
        &criteria,
        &count_operation,
    )
    .expect("conversation count");
    assert!(counts.total_count > page.returned_count as i64);
    assert!(counts.matching_message_count >= counts.total_count);
}

fn verify_wal_visible_read(root: &Path) {
    let workspace = create_synthetic_workspace(root, "wal-visible", "wal-visible", 32);
    let reader = open_workspace_db_for_read(&workspace.path).expect("WAL reader should open");
    let before_version = read_schema_version(&reader).expect("reader version");
    let writer = Connection::open(workspace.path.join("index.sqlite")).expect("WAL writer");
    writer
        .execute(
            "INSERT INTO messages (
                 id, folder_id, eml_path, subject, sender, recipients, date, body,
                 body_source, snippet, attachment_names, has_attachments,
                 normalized_subject, conversation_id, conversation_root_id,
                 thread_assignment_method
             ) VALUES (
                 33, 1, 'synthetic/wal/33', 'walvisible', 'sender@example.test',
                 'recipient@example.test', '2026-01-01T00:00:00+00:00', 'walvisible',
                 'text_plain', 'walvisible', '', 0, 'walvisible', 'wal-visible-33', 33,
                 'header'
             )",
            [],
        )
        .expect("WAL message should insert");
    writer
        .execute(
            "INSERT INTO messages_fts (
                 rowid, subject, sender, recipients, body, attachment_names
             ) SELECT id, subject, sender, recipients, body, attachment_names
                 FROM messages WHERE id = 33",
            [],
        )
        .expect("WAL FTS row should insert");
    drop(writer);
    let criteria = MessageSearchCriteria::from_inputs(Some("walvisible".to_string()), None)
        .expect("WAL criteria");
    assert_eq!(
        query_message_count(&reader, None, false, &criteria, None).expect("WAL-visible count"),
        1
    );
    assert_eq!(read_schema_version(&reader).unwrap(), before_version);
}

struct ProgressGate {
    callbacks: StdMutex<usize>,
    condition: Condvar,
}

const PROGRESS_GATE_THRESHOLD: usize = 256;

unsafe extern "C" fn progress_callback(context: *mut c_void) -> c_int {
    let gate = unsafe { &*(context.cast::<ProgressGate>()) };
    let mut callbacks = gate.callbacks.lock().expect("progress gate lock");
    *callbacks += 1;
    if *callbacks == PROGRESS_GATE_THRESHOLD {
        gate.condition.notify_all();
    }
    0
}

fn measure_cancellation_latency(workspace: &ActiveWorkspace, iteration: usize) -> Duration {
    let registry = Arc::new(SearchCancellationRegistry::default());
    let window = format!("stress-cancel-{iteration}");
    let operation = registry
        .begin_operation(
            &window,
            71,
            "message-count-1",
            SearchOperationCategory::MessageCount,
        )
        .expect("cancellation operation");
    let connection =
        open_workspace_db_for_search(&workspace.path, &operation).expect("search connection");
    let gate = Arc::new(ProgressGate {
        callbacks: StdMutex::new(0),
        condition: Condvar::new(),
    });
    let raw_gate = Arc::into_raw(Arc::clone(&gate)).cast_mut().cast::<c_void>();
    let raw_gate_address = raw_gate as usize;
    unsafe {
        ffi::sqlite3_progress_handler(connection.handle(), 1, Some(progress_callback), raw_gate);
    }

    let worker = thread::spawn(move || {
        let raw_gate = raw_gate_address as *mut c_void;
        let result = connection.query_row(
            "WITH matching(rowid) AS (
                 SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?1
             )
             SELECT COUNT(*) FROM matching AS first CROSS JOIN matching AS second",
            params![STRESS_SEARCH_TERM],
            |row| row.get::<_, i64>(0),
        );
        unsafe {
            ffi::sqlite3_progress_handler(connection.handle(), 0, None, ptr::null_mut());
            drop(Arc::from_raw(raw_gate.cast::<ProgressGate>()));
        }
        let still_usable = connection
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .expect("interrupted connection should remain usable");
        (result, still_usable, operation)
    });

    let mut callbacks = gate.callbacks.lock().expect("progress wait lock");
    while *callbacks < PROGRESS_GATE_THRESHOLD {
        callbacks = gate.condition.wait(callbacks).expect("progress wait");
    }
    drop(callbacks);
    let started = Instant::now();
    let outcome = registry
        .cancel_generation(&window, 71)
        .expect("generation cancellation");
    let (result, still_usable, operation) = worker.join().expect("cancellation worker");
    let elapsed = started.elapsed();
    let error = result.expect_err("expensive synthetic query should be interrupted");
    assert!(is_sqlite_interrupt(&error));
    assert_eq!(still_usable, 1);
    assert!(operation.check_cancelled().is_err());
    assert_eq!(outcome.operations, 1);
    assert_eq!(outcome.handles, 1);
    elapsed
}

fn measure_large_workspace(workspace: &ActiveWorkspace, dataset_size: i64) {
    let selective =
        MessageSearchCriteria::from_inputs(Some(SELECTIVE_SEARCH_TERM.to_string()), None)
            .expect("selective criteria");
    let broad = MessageSearchCriteria::from_inputs(Some(STRESS_SEARCH_TERM.to_string()), None)
        .expect("broad criteria");
    let rank = MessageSearchCriteria::from_inputs(Some(RANK_SEARCH_TERM.to_string()), None)
        .expect("rank criteria");
    let codec = SearchCursorCodec::default();

    measure(dataset_size, "selective_first_page", 12, || {
        let conn = open_workspace_db_for_read(&workspace.path).expect("selective read");
        let page = query_messages_cursor_page(
            &conn,
            &workspace.path,
            &workspace.id,
            None,
            false,
            &selective,
            Some("newest"),
            Some(250),
            None,
            81,
            &codec,
            None,
        )
        .expect("selective first page");
        black_box(page.returned_count);
    });
    measure(dataset_size, "broad_first_page", 12, || {
        let conn = open_workspace_db_for_read(&workspace.path).expect("broad read");
        let page = query_messages_cursor_page(
            &conn,
            &workspace.path,
            &workspace.id,
            None,
            false,
            &broad,
            Some("newest"),
            Some(250),
            None,
            82,
            &codec,
            None,
        )
        .expect("broad first page");
        black_box(page.returned_count);
    });
    measure(dataset_size, "exact_count", 10, || {
        let conn = open_workspace_db_for_read(&workspace.path).expect("count read");
        let count = query_message_count(&conn, None, false, &broad, None).expect("exact count");
        black_box(count);
    });
    measure(dataset_size, "relevance_first_page", 12, || {
        let conn = open_workspace_db_for_read(&workspace.path).expect("relevance read");
        let page = query_messages_cursor_page(
            &conn,
            &workspace.path,
            &workspace.id,
            None,
            false,
            &rank,
            Some("relevance"),
            Some(250),
            None,
            83,
            &codec,
            None,
        )
        .expect("relevance first page");
        black_box(page.returned_count);
    });

    let conn = open_workspace_db_for_read(&workspace.path).expect("cursor seed read");
    let first = query_messages_cursor_page(
        &conn,
        &workspace.path,
        &workspace.id,
        None,
        false,
        &broad,
        Some("newest"),
        Some(250),
        None,
        84,
        &codec,
        None,
    )
    .expect("cursor seed page");
    let next_cursor = first.next_cursor.expect("broad page should continue");
    drop(conn);
    measure(dataset_size, "next_cursor_page", 12, || {
        let conn = open_workspace_db_for_read(&workspace.path).expect("next-page read");
        let page = query_messages_cursor_page(
            &conn,
            &workspace.path,
            &workspace.id,
            None,
            false,
            &broad,
            Some("newest"),
            Some(250),
            Some(&next_cursor),
            84,
            &codec,
            None,
        )
        .expect("next cursor page");
        black_box(page.returned_count);
    });

    let mut cancellation_samples = (0..5)
        .map(|iteration| measure_cancellation_latency(workspace, iteration))
        .collect::<Vec<_>>();
    let p50 = percentile_ms(&mut cancellation_samples, 0.50);
    let p95 = percentile_ms(&mut cancellation_samples, 0.95);
    println!(
        "SEARCH_STRESS dataset_rows={dataset_size} metric=cancellation iterations=5 p50_ms={p50:.3} p95_ms={p95:.3}"
    );
}

#[test]
#[ignore = "deterministic Search 2.0 stress and performance gate"]
fn search_2_stabilization_stress_gate() {
    let root = StressRoot::new();
    verify_wal_visible_read(&root.path);

    for message_count in [10_000_i64, 100_000] {
        let started = Instant::now();
        let workspace = create_synthetic_workspace(
            &root.path,
            &format!("dataset-{message_count}"),
            &format!("workspace-{message_count}"),
            message_count,
        );
        let build_ms = started.elapsed().as_secs_f64() * 1_000.0;
        println!(
            "SEARCH_STRESS dataset_rows={message_count} metric=fixture_build iterations=1 p50_ms={build_ms:.3} p95_ms={build_ms:.3}"
        );
        let conn = open_workspace_db_for_read(&workspace.path).expect("dataset read");
        let criteria =
            MessageSearchCriteria::from_inputs(Some(STRESS_SEARCH_TERM.to_string()), None)
                .expect("dataset criteria");
        assert_eq!(
            query_message_count(&conn, None, false, &criteria, None).expect("dataset count"),
            message_count
        );
        drop(conn);
        if message_count == 10_000 {
            verify_cursor_and_context_correctness(&workspace, message_count as usize);
            verify_conversation_page_and_count(&workspace);
        }
    }

    let large_build_started = Instant::now();
    let large =
        create_synthetic_workspace(&root.path, "dataset-250000", "workspace-250000", 250_000);
    let large_build_ms = large_build_started.elapsed().as_secs_f64() * 1_000.0;
    println!(
        "SEARCH_STRESS dataset_rows=250000 metric=fixture_build iterations=1 p50_ms={large_build_ms:.3} p95_ms={large_build_ms:.3}"
    );
    measure_large_workspace(&large, 250_000);

    let multi = (0..3)
        .map(|index| {
            create_synthetic_workspace(
                &root.path,
                &format!("multi-{index}"),
                &format!("multi-workspace-{index}"),
                10_000,
            )
        })
        .collect::<Vec<_>>();
    verify_multi_workspace_behavior(&multi);
    let multi_criteria =
        MessageSearchCriteria::from_inputs(Some(STRESS_SEARCH_TERM.to_string()), None)
            .expect("multi benchmark criteria");
    let registry = Arc::new(SearchCancellationRegistry::default());
    measure(30_000, "multi_workspace_first_page", 7, || {
        static NEXT_OPERATION: AtomicUsize = AtomicUsize::new(0);
        let sequence = NEXT_OPERATION.fetch_add(1, Ordering::Relaxed) + 1;
        let operation = registry
            .begin_operation(
                "stress-multi-benchmark",
                91,
                &format!("message-page-{sequence}"),
                SearchOperationCategory::MessagePage,
            )
            .expect("multi benchmark operation");
        let page = query_multi_workspace_message_page(
            multi.clone(),
            &multi_criteria,
            "newest",
            Some(250),
            Some(0),
            &operation,
        )
        .expect("multi benchmark page");
        black_box(page.returned_count);
    });
}
