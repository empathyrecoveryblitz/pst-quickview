use super::*;

struct SyntheticImportCorpus {
    root: PathBuf,
    workspace: PathBuf,
    extract_dir: PathBuf,
    source: PathBuf,
    identity: PstIdentity,
}

type ConversationSnapshot = Vec<(
    i64,
    Option<String>,
    Option<i64>,
    Option<i64>,
    String,
    String,
)>;

fn temporary_import_integrity_directory(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = env::temp_dir().join(format!(
        "pst-quickview-import-integrity-{name}-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).expect("temporary import-integrity root should be created");
    directory
}

fn write_synthetic_message(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("synthetic message needs a parent"))
        .expect("synthetic message parent should be created");
    fs::write(path, bytes).expect("synthetic message should be written");
}

fn identical_message_with_attachment() -> &'static [u8] {
    concat!(
        "From: Fixture Sender <sender@fixture.invalid>\r\n",
        "To: fixture-recipient@fixture.invalid\r\n",
        "Subject: Repeated synthetic subject\r\n",
        "Date: Thu, 01 Jan 2026 00:00:00 +0000\r\n",
        "Message-ID: <repeated-message@fixture.invalid>\r\n",
        "MIME-Version: 1.0\r\n",
        "Content-Type: multipart/mixed; boundary=\"fixture-identical-boundary\"\r\n",
        "\r\n",
        "--fixture-identical-boundary\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "\r\n",
        "Repeated synthetic body.\r\n",
        "--fixture-identical-boundary\r\n",
        "Content-Type: application/octet-stream; name=\"fixture-identical.bin\"\r\n",
        "Content-Disposition: attachment; filename=\"fixture-identical.bin\"\r\n",
        "Content-Transfer-Encoding: base64\r\n",
        "\r\n",
        "AQIDBA==\r\n",
        "--fixture-identical-boundary--\r\n",
    )
    .as_bytes()
}

fn legitimate_no_subject_message() -> &'static [u8] {
    concat!(
        "From: No Subject Fixture <no-subject@fixture.invalid>\r\n",
        "To: fixture-recipient@fixture.invalid\r\n",
        "Date: Fri, 02 Jan 2026 00:00:00 +0000\r\n",
        "Message-ID: <no-subject@fixture.invalid>\r\n",
        "Content-Type: text/plain; charset=utf-8\r\n",
        "\r\n",
        "Synthetic body without a Subject header.\r\n",
    )
    .as_bytes()
}

fn legitimate_empty_body_message_with_attachment() -> &'static [u8] {
    concat!(
        "From: Empty Body Fixture <empty-body@fixture.invalid>\r\n",
        "To: fixture-recipient@fixture.invalid\r\n",
        "Subject: Synthetic empty body\r\n",
        "Date: Sat, 03 Jan 2026 00:00:00 +0000\r\n",
        "Message-ID: <empty-body@fixture.invalid>\r\n",
        "MIME-Version: 1.0\r\n",
        "Content-Type: multipart/mixed; boundary=\"fixture-empty-body-boundary\"\r\n",
        "\r\n",
        "--fixture-empty-body-boundary\r\n",
        "Content-Type: application/octet-stream; name=\"fixture-empty-body.dat\"\r\n",
        "Content-Disposition: attachment; filename=\"fixture-empty-body.dat\"\r\n",
        "Content-Transfer-Encoding: base64\r\n",
        "\r\n",
        "BQYHCA==\r\n",
        "--fixture-empty-body-boundary--\r\n",
    )
    .as_bytes()
}

fn readpst_empty_envelope(item: usize, include_status: bool) -> Vec<u8> {
    let boundary = format!("--boundary-LibPST-iamunique-{item}_-_-");
    let status = if include_status { "Status: RO\n" } else { "" };
    format!(
        "{status}From: <MAILER-DAEMON>\nSubject: \nMIME-Version: 1.0\nContent-Type: multipart/mixed;\n\tboundary=\"{boundary}\"\n\n\n--{boundary}--\n\n"
    )
    .into_bytes()
}

fn uncertain_readpst_envelope(item: usize) -> Vec<u8> {
    let boundary = format!("--boundary-LibPST-iamunique-{item}_-_-");
    format!(
        "From: <MAILER-DAEMON>\nSubject: \nX-Fixture-Evidence: retained\nMIME-Version: 1.0\nContent-Type: multipart/mixed;\n\tboundary=\"{boundary}\"\n\n\n--{boundary}--\n\n"
    )
    .into_bytes()
}

fn create_synthetic_import_corpus(name: &str) -> SyntheticImportCorpus {
    let root = temporary_import_integrity_directory(name);
    let source = root.join("source.pst");
    fs::write(&source, b"!BDNsynthetic import-integrity source")
        .expect("synthetic PST source should be written");
    let identity = identify_pst(&source).expect("synthetic PST identity should be available");

    let workspace = root.join("workspace");
    let extract_dir = workspace.join("extracted");
    create_private_directory_all(&workspace.join("logs"))
        .expect("synthetic private log directory should be created");
    initialize_import_log(&workspace, "synthetic-readpst")
        .expect("synthetic import log should initialize");

    write_synthetic_message(
        &extract_dir.join("A-Inbox/1.eml"),
        identical_message_with_attachment(),
    );
    write_synthetic_message(
        &extract_dir.join("A-Inbox/2.eml"),
        identical_message_with_attachment(),
    );
    write_synthetic_message(
        &extract_dir.join("B-Archive/3.eml"),
        legitimate_no_subject_message(),
    );
    write_synthetic_message(
        &extract_dir.join("B-Archive/4.eml"),
        legitimate_empty_body_message_with_attachment(),
    );
    write_synthetic_message(
        &extract_dir.join("C-Uncertain/5.eml"),
        &uncertain_readpst_envelope(5),
    );

    for (item, canonical_parent) in [
        (6, "A-Inbox/1.eml"),
        (7, "A-Inbox/2.eml"),
        (8, "B-Archive/3.eml"),
        (9, "B-Archive/4.eml"),
        (10, "C-Uncertain/5.eml"),
    ] {
        write_synthetic_message(
            &extract_dir.join(format!("{canonical_parent}-synthetic-empty-{item}.eml")),
            &readpst_empty_envelope(item, item % 2 == 0),
        );
    }

    assert_eq!(
        count_eml_files(&extract_dir).expect("synthetic candidates should be countable"),
        10
    );

    SyntheticImportCorpus {
        root,
        workspace,
        extract_dir,
        source,
        identity,
    }
}

fn open_synthetic_index(corpus: &SyntheticImportCorpus) -> (PathBuf, Connection) {
    let index_path = corpus.workspace.join("index.tmp.sqlite");
    let mut conn = open_sqlite_write_connection(&index_path)
        .expect("synthetic import-integrity index should open");
    initialize_schema(&conn).expect("synthetic import-integrity schema should initialize");
    let discovery = discover_eml_candidates(&corpus.extract_dir)
        .expect("synthetic candidate discovery should succeed");
    prepare_import_metadata(
        &mut conn,
        &corpus.identity,
        &corpus.identity.workspace_id,
        10,
        &corpus.workspace,
        WorkspaceLocationMode::AppSupport,
        "synthetic-readpst",
        "synthetic-readpst-version",
        false,
        true,
        &discovery,
    )
    .expect("synthetic import metadata should initialize");
    (index_path, conn)
}

fn prepare_synthetic_resume(corpus: &SyntheticImportCorpus, conn: &mut Connection) {
    let discovery = discover_eml_candidates(&corpus.extract_dir)
        .expect("synthetic resume discovery should succeed");
    prepare_import_metadata(
        conn,
        &corpus.identity,
        &corpus.identity.workspace_id,
        10,
        &corpus.workspace,
        WorkspaceLocationMode::AppSupport,
        "readpst not run during synthetic resume",
        "readpst not run during synthetic resume",
        true,
        false,
        &discovery,
    )
    .expect("synthetic resume metadata should validate");
}

fn run_synthetic_index<C>(
    corpus: &SyntheticImportCorpus,
    conn: &mut Connection,
    batch_size: usize,
    is_cancelled: C,
) -> AppResult<IndexStats>
where
    C: FnMut(&IndexStats) -> bool,
{
    let discovery = discover_eml_candidates(&corpus.extract_dir)?;
    stream_index_eml_files_with_control(
        &corpus.workspace,
        &corpus.extract_dir,
        conn,
        &discovery,
        batch_size,
        "Synthetic import cancellation.",
        is_cancelled,
        |_| {},
    )
}

fn scalar_count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0))
        .expect("synthetic count query should succeed")
}

fn message_id_snapshot(conn: &Connection) -> Vec<(String, i64)> {
    let mut statement = conn
        .prepare("SELECT eml_path, id FROM messages ORDER BY eml_path")
        .expect("message identity snapshot should prepare");
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("message identity snapshot should query");
    rows.map(|row| row.expect("message identity row should decode"))
        .collect()
}

fn folder_assignment_snapshot(conn: &Connection) -> Vec<(String, i64)> {
    let mut statement = conn
        .prepare("SELECT eml_path, folder_id FROM messages ORDER BY eml_path")
        .expect("folder assignment snapshot should prepare");
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("folder assignment snapshot should query");
    rows.map(|row| row.expect("folder assignment row should decode"))
        .collect()
}

fn conversation_snapshot(conn: &Connection) -> ConversationSnapshot {
    let mut statement = conn
        .prepare(
            "SELECT id,
                    conversation_id,
                    conversation_parent_id,
                    conversation_root_id,
                    thread_assignment_method,
                    COALESCE(thread_warning, '')
               FROM messages
              ORDER BY id",
        )
        .expect("conversation snapshot should prepare");
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .expect("conversation snapshot should query");
    rows.map(|row| row.expect("conversation snapshot row should decode"))
        .collect()
}

fn assert_canonical_index_shape(conn: &Connection) {
    assert_eq!(scalar_count(conn, "SELECT COUNT(*) FROM messages"), 10);
    assert_eq!(scalar_count(conn, "SELECT COUNT(*) FROM messages_fts"), 10);
    assert_eq!(scalar_count(conn, "SELECT COUNT(*) FROM attachments"), 3);
    assert_eq!(
        scalar_count(
            conn,
            "SELECT COUNT(*)
               FROM messages
               JOIN folders ON folders.id = messages.folder_id",
        ),
        10
    );
    assert_eq!(scalar_count(conn, "SELECT COUNT(*) FROM folders"), 4);
    assert_eq!(
        scalar_count(conn, "SELECT COUNT(DISTINCT path) FROM folders"),
        4
    );
    assert_eq!(
        scalar_count(
            conn,
            "SELECT COUNT(*)
               FROM (
                    SELECT message_id, attachment_index
                      FROM attachments
                     GROUP BY message_id, attachment_index
               )",
        ),
        3
    );

    let identical_ids = {
        let mut statement = conn
            .prepare(
                "SELECT id
                   FROM messages
                  WHERE eml_path IN ('A-Inbox/1.eml', 'A-Inbox/2.eml')
                  ORDER BY eml_path",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
    };
    assert_eq!(identical_ids.len(), 2);
    assert_ne!(identical_ids[0], identical_ids[1]);
    assert_eq!(
        scalar_count(
            conn,
            "SELECT COUNT(*)
               FROM messages
              WHERE eml_path = 'B-Archive/3.eml'
                AND COALESCE(subject, '') = ''
                AND COALESCE(body, '') <> ''",
        ),
        1
    );
    assert_eq!(
        scalar_count(
            conn,
            "SELECT COUNT(*)
               FROM messages
              WHERE eml_path = 'B-Archive/4.eml'
                AND COALESCE(body, '') = ''
                AND has_attachments = 1",
        ),
        1
    );
    assert_eq!(
        scalar_count(
            conn,
            "SELECT COUNT(*)
               FROM messages
              WHERE eml_path = 'C-Uncertain/5.eml'
                AND COALESCE(subject, '') = ''
                AND sender = '<MAILER-DAEMON>'",
        ),
        1
    );
    assert_eq!(
        scalar_count(
            conn,
            "SELECT COUNT(*)
               FROM messages
              WHERE eml_path LIKE '%-synthetic-empty-%.eml'
                AND COALESCE(subject, '') = ''
                AND sender = '<MAILER-DAEMON>'",
        ),
        5,
        "uncertain blank candidates must remain indexed and visible"
    );
}

#[test]
fn first_clean_import_retains_blank_candidates_and_persists_zero_suppression() {
    let corpus = create_synthetic_import_corpus("first-clean-import");
    let source_before = file_identity(&corpus.source, "source PST").unwrap();
    let (_index_path, mut conn) = open_synthetic_index(&corpus);

    let stats = run_synthetic_index(&corpus, &mut conn, 2, |_| false)
        .expect("first clean synthetic import should finish");
    assert_eq!(
        stats,
        IndexStats {
            discovered: 10,
            processed: 10,
            indexed: 10,
            inserted: 10,
            skipped_existing: 0,
            suppressed_empty_artifacts: 0,
            error_count: 0,
            attachments_indexed: 3,
            last_error: None,
        }
    );
    assert_canonical_index_shape(&conn);

    apply_conversation_index(&mut conn, &corpus.identity.workspace_id, || false)
        .expect("conversation metadata should be complete before projection");
    finalize_import_index_metadata(&mut conn, &stats)
        .expect("completed SQLite metadata should be finalized before projection");
    mark_workspace_complete_file_only(
        &corpus.workspace,
        &corpus.identity,
        WorkspaceLocationMode::AppSupport,
        &stats,
    )
    .expect("completed workspace metadata should be projected once");
    let workspace_metadata: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(corpus.workspace.join(WORKSPACE_METADATA_FILE)).unwrap(),
    )
    .unwrap();
    for (key, expected) in [
        ("candidates_discovered", 10_u64),
        ("candidates_processed", 10),
        ("canonical_messages_inserted", 10),
        ("existing_canonical_messages_skipped", 0),
        ("structurally_empty_artifacts_suppressed", 0),
        ("parse_failures", 0),
        ("attachments_indexed", 3),
        ("canonical_messages_total", 10),
        ("message_count_indexed", 10),
    ] {
        assert_eq!(workspace_metadata[key].as_u64(), Some(expected));
    }
    assert_eq!(workspace_metadata["import_status"], "complete");
    assert_eq!(
        workspace_metadata["conversation_schema_version"],
        CONVERSATION_SCHEMA_VERSION
    );

    for (key, expected) in [
        ("candidates_discovered", "10"),
        ("candidates_processed", "10"),
        ("canonical_messages_inserted", "10"),
        ("existing_canonical_messages_skipped", "0"),
        ("structurally_empty_artifacts_suppressed", "0"),
        ("parse_failures", "0"),
        ("attachments_indexed", "3"),
        ("canonical_messages_total", "10"),
        ("message_count_indexed", "10"),
    ] {
        assert_eq!(
            metadata_value(&conn, key).unwrap().as_deref(),
            Some(expected),
            "unexpected persisted import-integrity counter for {key}"
        );
    }
    assert_eq!(
        file_identity(&corpus.source, "source PST").unwrap(),
        source_before
    );
    let import_log = fs::read_to_string(import_log_path(&corpus.workspace)).unwrap();
    assert!(!import_log.contains("Extraction artifact suppressed."));
    for private_value in [
        "A-Inbox",
        "synthetic-empty",
        ".eml",
        "@fixture.invalid",
        "Repeated synthetic subject",
        "Repeated synthetic body",
    ] {
        assert!(!import_log.contains(private_value));
    }

    drop(conn);
    fs::remove_dir_all(corpus.root).expect("synthetic clean-import corpus should be removed");
}

#[test]
fn interrupted_import_resumes_idempotently_across_reresume_and_reopen() {
    let corpus = create_synthetic_import_corpus("resume-lifecycle");
    let source_before = file_identity(&corpus.source, "source PST")
        .expect("synthetic source identity should be captured");
    assert_eq!(
        fs::read(corpus.extract_dir.join("A-Inbox/1.eml")).unwrap(),
        fs::read(corpus.extract_dir.join("A-Inbox/2.eml")).unwrap(),
        "two byte-identical source records must remain distinct by relative identity"
    );

    let (temp_index, mut conn) = open_synthetic_index(&corpus);
    let cancellation = run_synthetic_index(&corpus, &mut conn, 4, |stats| stats.processed >= 4)
        .expect_err("synthetic indexing should stop after the first committed batch");
    assert_eq!(cancellation.message, "Synthetic import cancellation.");
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM messages"), 4);
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM attachments"), 2);
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM messages_fts"), 4);
    let committed_ids = message_id_snapshot(&conn);

    prepare_synthetic_resume(&corpus, &mut conn);
    let resumed = run_synthetic_index(&corpus, &mut conn, 4, |_| false)
        .expect("same-database resume should finish");
    assert_eq!(
        resumed,
        IndexStats {
            discovered: 10,
            processed: 10,
            indexed: 10,
            inserted: 6,
            skipped_existing: 4,
            suppressed_empty_artifacts: 0,
            error_count: 0,
            attachments_indexed: 1,
            last_error: None,
        }
    );
    assert_eq!(&message_id_snapshot(&conn)[..4], committed_ids.as_slice());
    assert_canonical_index_shape(&conn);

    apply_conversation_index(&mut conn, &corpus.identity.workspace_id, || false)
        .expect("first conversation build should succeed");
    let first_conversations = conversation_snapshot(&conn);
    apply_conversation_index(&mut conn, &corpus.identity.workspace_id, || false)
        .expect("repeated conversation build should succeed");
    assert_eq!(conversation_snapshot(&conn), first_conversations);

    let stable_ids = message_id_snapshot(&conn);
    let stable_folder_assignments = folder_assignment_snapshot(&conn);
    prepare_synthetic_resume(&corpus, &mut conn);
    let repeated = run_synthetic_index(&corpus, &mut conn, 4, |_| false)
        .expect("repeated same-database resume should finish");
    assert_eq!(repeated.inserted, 0);
    assert_eq!(repeated.skipped_existing, 10);
    assert_eq!(repeated.suppressed_empty_artifacts, 0);
    assert_eq!(repeated.attachments_indexed, 0);
    assert_eq!(repeated.indexed, 10);
    assert_eq!(message_id_snapshot(&conn), stable_ids);
    assert_eq!(folder_assignment_snapshot(&conn), stable_folder_assignments);
    assert_canonical_index_shape(&conn);

    finalize_import_index_metadata(&mut conn, &repeated)
        .expect("synthetic completed metadata should be finalized");
    optimize_then_checked_truncate(&conn).expect("synthetic index should checkpoint before reopen");
    drop(conn);
    let promotion_discovery = discover_eml_candidates(&corpus.extract_dir).unwrap();
    promote_completed_index(
        &corpus.workspace,
        &temp_index,
        &[corpus.identity.workspace_id.as_str()],
        &corpus.identity.content_fingerprint,
        &promotion_discovery,
    )
    .expect("synthetic index should move into place through production promotion");

    let mut reopened = open_workspace_db_for_upgrade(&corpus.workspace)
        .expect("synthetic workspace should reopen");
    prepare_synthetic_resume(&corpus, &mut reopened);
    let rediscovered = run_synthetic_index(&corpus, &mut reopened, 4, |_| false)
        .expect("reopened rediscovery should finish");
    assert_eq!(rediscovered.inserted, 0);
    assert_eq!(rediscovered.skipped_existing, 10);
    assert_eq!(rediscovered.suppressed_empty_artifacts, 0);
    assert_eq!(message_id_snapshot(&reopened), stable_ids);
    assert_eq!(
        folder_assignment_snapshot(&reopened),
        stable_folder_assignments
    );
    apply_conversation_index(&mut reopened, &corpus.identity.workspace_id, || false)
        .expect("reopened conversation build should succeed");
    assert_eq!(conversation_snapshot(&reopened), first_conversations);
    assert_canonical_index_shape(&reopened);
    assert_eq!(
        file_identity(&corpus.source, "source PST").unwrap(),
        source_before,
        "source PST identity must remain unchanged through interruption and resume"
    );

    drop(reopened);
    fs::remove_dir_all(corpus.root).expect("synthetic lifecycle corpus should be removed");
}

#[test]
fn transaction_abort_rolls_back_current_batch_and_retry_is_idempotent() {
    let corpus = create_synthetic_import_corpus("transaction-retry");
    let source_before = file_identity(&corpus.source, "source PST").unwrap();
    let (_index_path, mut conn) = open_synthetic_index(&corpus);
    conn.execute_batch(
        "CREATE TEMP TRIGGER synthetic_import_abort
         BEFORE INSERT ON messages
         WHEN NEW.eml_path = 'B-Archive/4.eml'
         BEGIN
             SELECT RAISE(ABORT, 'synthetic transaction failure');
         END;",
    )
    .expect("temporary synthetic failure trigger should install");

    let error = run_synthetic_index(&corpus, &mut conn, 4, |_| false)
        .expect_err("synthetic row failure should abort the current batch");
    assert!(error.to_string().contains("synthetic transaction failure"));
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM messages"), 4);
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM attachments"), 2);
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM messages_fts"), 4);
    assert_eq!(
        scalar_count(
            &conn,
            "SELECT COUNT(*) FROM messages WHERE eml_path = 'B-Archive/3.eml'",
        ),
        0,
        "the earlier row in the failed transaction must roll back"
    );
    let committed_ids = message_id_snapshot(&conn);

    conn.execute_batch("DROP TRIGGER synthetic_import_abort")
        .expect("temporary synthetic failure trigger should be removed");
    prepare_synthetic_resume(&corpus, &mut conn);
    let retried = run_synthetic_index(&corpus, &mut conn, 4, |_| false)
        .expect("retry after transaction rollback should finish");
    assert_eq!(retried.inserted, 6);
    assert_eq!(retried.skipped_existing, 4);
    assert_eq!(retried.suppressed_empty_artifacts, 0);
    assert_eq!(retried.attachments_indexed, 1);
    assert_eq!(&message_id_snapshot(&conn)[..4], committed_ids.as_slice());
    assert_canonical_index_shape(&conn);
    assert_eq!(
        file_identity(&corpus.source, "source PST").unwrap(),
        source_before
    );

    drop(conn);
    fs::remove_dir_all(corpus.root).expect("synthetic transaction corpus should be removed");
}

#[test]
fn counter_snapshot_failure_rolls_back_rows_and_counters_together() {
    let corpus = create_synthetic_import_corpus("counter-transaction-retry");
    let source_before = file_identity(&corpus.source, "source PST").unwrap();
    let (_index_path, mut conn) = open_synthetic_index(&corpus);
    conn.execute_batch(
        "CREATE TEMP TRIGGER synthetic_counter_abort
         BEFORE INSERT ON import_metadata
         WHEN NEW.key = 'candidates_processed' AND NEW.value = '8'
         BEGIN
             SELECT RAISE(ABORT, 'synthetic counter snapshot failure');
         END;",
    )
    .expect("temporary counter failure trigger should install");

    let error = run_synthetic_index(&corpus, &mut conn, 4, |_| false)
        .expect_err("the second batch counter snapshot should fail");
    assert!(error
        .to_string()
        .contains("synthetic counter snapshot failure"));
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM messages"), 4);
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM attachments"), 2);
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM messages_fts"), 4);
    for (key, expected) in [
        ("candidates_processed", "4"),
        ("canonical_messages_inserted", "4"),
        ("existing_canonical_messages_skipped", "0"),
        ("structurally_empty_artifacts_suppressed", "0"),
        ("attachments_indexed", "2"),
        ("canonical_messages_total", "4"),
        ("message_count_indexed", "4"),
    ] {
        assert_eq!(
            metadata_value(&conn, key).unwrap().as_deref(),
            Some(expected)
        );
    }

    conn.execute_batch("DROP TRIGGER synthetic_counter_abort")
        .expect("temporary counter failure trigger should be removed");
    prepare_synthetic_resume(&corpus, &mut conn);
    let retried = run_synthetic_index(&corpus, &mut conn, 4, |_| false)
        .expect("retry after counter rollback should finish");
    assert_eq!(retried.inserted, 6);
    assert_eq!(retried.skipped_existing, 4);
    assert_eq!(retried.suppressed_empty_artifacts, 0);
    assert_canonical_index_shape(&conn);
    assert_eq!(
        file_identity(&corpus.source, "source PST").unwrap(),
        source_before
    );

    drop(conn);
    fs::remove_dir_all(corpus.root).expect("synthetic counter corpus should be removed");
}

#[test]
fn direct_canonical_conflict_skips_all_child_inserts() {
    let corpus = create_synthetic_import_corpus("direct-conflict");
    let mut conn = Connection::open_in_memory().unwrap();
    initialize_schema(&conn).unwrap();

    let first = parse_eml(&corpus.extract_dir.join("A-Inbox/1.eml")).unwrap();
    let second = parse_eml(&corpus.extract_dir.join("A-Inbox/2.eml")).unwrap();
    let transaction = conn.transaction().unwrap();
    let mut folder_cache = HashMap::new();
    let folder_id = ensure_folder(&transaction, &mut folder_cache, "A-Inbox").unwrap();
    let first_id = match insert_message(&transaction, folder_id, "A-Inbox/1.eml", first).unwrap() {
        InsertMessageOutcome::Inserted { message_id, .. } => message_id,
        InsertMessageOutcome::Existing { .. } => panic!("first canonical row must be inserted"),
    };
    let existing_id = match insert_message(&transaction, folder_id, "A-Inbox/1.eml", second)
        .expect("the exact canonical conflict should be handled")
    {
        InsertMessageOutcome::Existing { message_id } => message_id,
        InsertMessageOutcome::Inserted { .. } => panic!("the canonical row must not duplicate"),
    };
    transaction.commit().unwrap();

    assert_eq!(existing_id, first_id);
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM messages"), 1);
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM attachments"), 1);
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM messages_fts"), 1);
    assert_eq!(scalar_count(&conn, "SELECT COUNT(*) FROM folders"), 2);

    drop(conn);
    fs::remove_dir_all(corpus.root).expect("synthetic conflict corpus should be removed");
}

#[test]
fn resume_preflight_is_read_only_and_legacy_provenance_is_explicitly_adopted() {
    let corpus = create_synthetic_import_corpus("resume-preflight");
    let (index_path, mut conn) = open_synthetic_index(&corpus);
    let discovery = discover_eml_candidates(&corpus.extract_dir).unwrap();

    preflight_resume_index(
        &index_path,
        &corpus.identity,
        &corpus.identity.workspace_id,
        &corpus.extract_dir,
        &discovery,
    )
    .expect("matching current provenance should preflight");

    let original_metadata_count = scalar_count(&conn, "SELECT COUNT(*) FROM import_metadata");
    assert!(preflight_resume_index(
        &index_path,
        &corpus.identity,
        &"f".repeat(64),
        &corpus.extract_dir,
        &discovery,
    )
    .is_err());
    assert_eq!(
        scalar_count(&conn, "SELECT COUNT(*) FROM import_metadata"),
        original_metadata_count,
        "a rejected read-only preflight must not rewrite metadata"
    );

    let other_source = corpus.root.join("other-source.pst");
    fs::write(&other_source, b"!BDNdifferent synthetic source").unwrap();
    let other_identity = identify_pst(&other_source).unwrap();
    assert!(preflight_resume_index(
        &index_path,
        &other_identity,
        &corpus.identity.workspace_id,
        &corpus.extract_dir,
        &discovery,
    )
    .is_err());

    set_metadata_value(&conn, "importer_generation", "unsupported-generation").unwrap();
    assert!(preflight_resume_index(
        &index_path,
        &corpus.identity,
        &corpus.identity.workspace_id,
        &corpus.extract_dir,
        &discovery,
    )
    .is_err());
    assert_eq!(
        metadata_value(&conn, "importer_generation")
            .unwrap()
            .as_deref(),
        Some("unsupported-generation")
    );
    set_metadata_value(&conn, "importer_generation", IMPORTER_GENERATION).unwrap();

    let original_candidate = corpus
        .extract_dir
        .join("C-Uncertain/5.eml-synthetic-empty-10.eml");
    let changed_candidate = corpus
        .extract_dir
        .join("C-Uncertain/5.eml-synthetic-empty-11.eml");
    fs::rename(&original_candidate, &changed_candidate).unwrap();
    let changed_discovery = discover_eml_candidates(&corpus.extract_dir).unwrap();
    assert_eq!(changed_discovery.count, discovery.count);
    assert_ne!(
        changed_discovery.identity_manifest,
        discovery.identity_manifest
    );
    assert!(preflight_resume_index(
        &index_path,
        &corpus.identity,
        &corpus.identity.workspace_id,
        &corpus.extract_dir,
        &changed_discovery,
    )
    .is_err());
    fs::rename(&changed_candidate, &original_candidate).unwrap();

    let original_bytes = fs::read(&original_candidate).unwrap();
    let mut changed_bytes = original_bytes.clone();
    changed_bytes.extend_from_slice(b"X-Synthetic-Change: yes\r\n");
    fs::write(&original_candidate, &changed_bytes).unwrap();
    let content_changed_discovery = discover_eml_candidates(&corpus.extract_dir).unwrap();
    assert_eq!(content_changed_discovery.count, discovery.count);
    assert_ne!(
        content_changed_discovery.identity_manifest,
        discovery.identity_manifest
    );
    assert!(preflight_resume_index(
        &index_path,
        &corpus.identity,
        &corpus.identity.workspace_id,
        &corpus.extract_dir,
        &content_changed_discovery,
    )
    .is_err());
    fs::write(&original_candidate, &original_bytes).unwrap();

    conn.execute_batch(&format!(
        "PRAGMA user_version = {};",
        SQLITE_SCHEMA_VERSION_CURRENT + 1
    ))
    .unwrap();
    assert!(preflight_resume_index(
        &index_path,
        &corpus.identity,
        &corpus.identity.workspace_id,
        &corpus.extract_dir,
        &discovery,
    )
    .is_err());
    conn.execute_batch(&format!(
        "PRAGMA user_version = {SQLITE_SCHEMA_VERSION_CURRENT};"
    ))
    .unwrap();

    let cancellation = run_synthetic_index(&corpus, &mut conn, 4, |stats| stats.processed >= 4)
        .expect_err("legacy compatibility fixture should stop after one committed batch");
    assert_eq!(cancellation.message, "Synthetic import cancellation.");
    let legacy_committed_ids = message_id_snapshot(&conn);
    assert_eq!(legacy_committed_ids.len(), 4);

    for key in [
        "canonical_import_identity_version",
        "importer_generation",
        "candidate_identity_manifest",
    ] {
        conn.execute("DELETE FROM import_metadata WHERE key = ?1", params![key])
            .unwrap();
    }
    set_metadata_value(&conn, "import_status", ImportStatus::Cancelled.as_str()).unwrap();
    preflight_resume_index(
        &index_path,
        &corpus.identity,
        &corpus.identity.workspace_id,
        &corpus.extract_dir,
        &discovery,
    )
    .expect("fully missing pre-Phase-1 provenance should use the explicit compatibility path");
    prepare_import_metadata(
        &mut conn,
        &corpus.identity,
        &corpus.identity.workspace_id,
        discovery.count,
        &corpus.workspace,
        WorkspaceLocationMode::AppSupport,
        "must-not-replace-original-readpst-path",
        "must-not-replace-original-readpst-version",
        true,
        false,
        &discovery,
    )
    .expect("validated legacy provenance should be adopted transactionally");
    assert_eq!(
        metadata_value(&conn, "legacy_import_provenance_adopted")
            .unwrap()
            .as_deref(),
        Some("true")
    );
    assert_eq!(
        metadata_value(&conn, "readpst_path").unwrap().as_deref(),
        Some("synthetic-readpst")
    );
    let legacy_resumed = run_synthetic_index(&corpus, &mut conn, 4, |_| false)
        .expect("adopted pre-Phase-1 partial rows should resume idempotently");
    assert_eq!(legacy_resumed.inserted, 6);
    assert_eq!(legacy_resumed.skipped_existing, 4);
    assert_eq!(legacy_resumed.suppressed_empty_artifacts, 0);
    assert_eq!(
        &message_id_snapshot(&conn)[..legacy_committed_ids.len()],
        legacy_committed_ids.as_slice()
    );
    assert_canonical_index_shape(&conn);
    preflight_resume_index(
        &index_path,
        &corpus.identity,
        &corpus.identity.legacy_workspace_id,
        &corpus.extract_dir,
        &discovery,
    )
    .expect("a selected legacy path may adopt the matching current source identity");
    prepare_import_metadata(
        &mut conn,
        &corpus.identity,
        &corpus.identity.legacy_workspace_id,
        discovery.count,
        &corpus.workspace,
        WorkspaceLocationMode::AppSupport,
        "must-not-replace-original-readpst-path",
        "must-not-replace-original-readpst-version",
        true,
        false,
        &discovery,
    )
    .expect("legacy workspace identity adoption should be transactional");
    assert_eq!(
        metadata_value(&conn, "workspace_id").unwrap().as_deref(),
        Some(corpus.identity.legacy_workspace_id.as_str())
    );
    assert_eq!(
        metadata_value(&conn, "legacy_workspace_identity_adopted")
            .unwrap()
            .as_deref(),
        Some("true")
    );

    drop(conn);
    fs::remove_dir_all(corpus.root).expect("synthetic preflight corpus should be removed");
}

#[test]
fn pristine_recovery_is_limited_to_the_partial_temp_index() {
    let corpus = create_synthetic_import_corpus("pristine-partial-only");
    let discovery = discover_eml_candidates(&corpus.extract_dir).unwrap();
    let temp_index = corpus.workspace.join("index.tmp.sqlite");
    File::create(&temp_index).expect("zero-length synthetic partial index should be created");
    assert_eq!(
        preflight_resume_index(
            &temp_index,
            &corpus.identity,
            &corpus.identity.workspace_id,
            &corpus.extract_dir,
            &discovery,
        )
        .unwrap(),
        ResumeIndexPreflight::Pristine
    );
    remove_sqlite_file_set(&temp_index).unwrap();

    let final_index = corpus.workspace.join("index.sqlite");
    File::create(&final_index).expect("zero-length synthetic final index should be created");
    assert!(preflight_resume_index(
        &final_index,
        &corpus.identity,
        &corpus.identity.workspace_id,
        &corpus.extract_dir,
        &discovery,
    )
    .is_err());
    remove_sqlite_file_set(&final_index).unwrap();

    let conn = open_sqlite_write_connection(&temp_index).unwrap();
    initialize_schema(&conn).unwrap();
    drop(conn);
    assert_eq!(
        preflight_resume_index(
            &temp_index,
            &corpus.identity,
            &corpus.identity.workspace_id,
            &corpus.extract_dir,
            &discovery,
        )
        .unwrap(),
        ResumeIndexPreflight::Pristine
    );

    let conn = open_sqlite_write_connection(&temp_index).unwrap();
    conn.execute(
        "INSERT INTO folders (parent_id, path, name) VALUES (NULL, '', 'PST')",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(preflight_resume_index(
        &temp_index,
        &corpus.identity,
        &corpus.identity.workspace_id,
        &corpus.extract_dir,
        &discovery,
    )
    .is_err());

    fs::remove_dir_all(corpus.root).expect("synthetic pristine corpus should be removed");
}

#[test]
fn checked_checkpoint_rejects_busy_wal_without_discarding_committed_data() {
    let root = temporary_import_integrity_directory("busy-checkpoint");
    let index_path = root.join("index.sqlite");
    let writer = open_sqlite_write_connection(&index_path).unwrap();
    initialize_schema(&writer).unwrap();
    checked_truncate_checkpoint(&writer).unwrap();

    let reader = open_sqlite_read_only_connection(&index_path).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let _snapshot_count = reader
        .query_row("SELECT COUNT(*) FROM import_metadata", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    set_metadata_value(&writer, "synthetic_busy_key", "committed").unwrap();

    assert!(checked_truncate_checkpoint(&writer).is_err());
    assert_eq!(
        metadata_value(&writer, "synthetic_busy_key")
            .unwrap()
            .as_deref(),
        Some("committed")
    );
    reader.execute_batch("ROLLBACK").unwrap();
    drop(reader);
    checked_truncate_checkpoint(&writer).unwrap();

    drop(writer);
    fs::remove_dir_all(root).expect("synthetic checkpoint corpus should be removed");
}

#[test]
fn busy_main_index_blocks_promotion_and_preserves_both_databases() {
    let corpus = create_synthetic_import_corpus("busy-promotion");
    let source_before = file_identity(&corpus.source, "source PST").unwrap();
    let (temp_index, mut conn) = open_synthetic_index(&corpus);
    let stats = run_synthetic_index(&corpus, &mut conn, 4, |_| false).unwrap();
    apply_conversation_index(&mut conn, &corpus.identity.workspace_id, || false).unwrap();
    finalize_import_index_metadata(&mut conn, &stats).unwrap();
    optimize_then_checked_truncate(&conn).unwrap();
    drop(conn);

    let discovery = discover_eml_candidates(&corpus.extract_dir).unwrap();
    let allowed_workspace_ids = [corpus.identity.workspace_id.as_str()];
    promote_completed_index(
        &corpus.workspace,
        &temp_index,
        &allowed_workspace_ids,
        &corpus.identity.content_fingerprint,
        &discovery,
    )
    .unwrap();
    let main_index = corpus.workspace.join("index.sqlite");
    fs::copy(&main_index, &temp_index).expect("synthetic replacement index should be copied");

    let reader = open_sqlite_read_only_connection(&main_index).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    assert_eq!(scalar_count(&reader, "SELECT COUNT(*) FROM messages"), 10);
    let writer = open_sqlite_write_connection(&main_index).unwrap();
    set_metadata_value(&writer, "synthetic_busy_promotion", "committed").unwrap();
    drop(writer);

    assert!(promote_completed_index(
        &corpus.workspace,
        &temp_index,
        &allowed_workspace_ids,
        &corpus.identity.content_fingerprint,
        &discovery,
    )
    .is_err());
    assert!(main_index.is_file());
    assert!(temp_index.is_file());
    assert_eq!(scalar_count(&reader, "SELECT COUNT(*) FROM messages"), 10);
    let preserved_main = open_sqlite_read_only_connection(&main_index).unwrap();
    assert_eq!(
        metadata_value(&preserved_main, "synthetic_busy_promotion")
            .unwrap()
            .as_deref(),
        Some("committed")
    );
    assert_eq!(
        metadata_value(&preserved_main, "import_status")
            .unwrap()
            .as_deref(),
        Some("complete")
    );
    drop(preserved_main);

    reader.execute_batch("ROLLBACK").unwrap();
    drop(reader);
    promote_completed_index(
        &corpus.workspace,
        &temp_index,
        &allowed_workspace_ids,
        &corpus.identity.content_fingerprint,
        &discovery,
    )
    .expect("promotion should succeed after the busy reader closes");
    assert!(main_index.is_file());
    assert!(!temp_index.exists());
    mark_workspace_complete_file_only(
        &corpus.workspace,
        &corpus.identity,
        WorkspaceLocationMode::AppSupport,
        &stats,
    )
    .expect("JSON counters should project only after successful promotion");
    let workspace_metadata: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(corpus.workspace.join(WORKSPACE_METADATA_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(workspace_metadata["canonical_messages_total"], 10);
    assert_eq!(
        workspace_metadata["structurally_empty_artifacts_suppressed"],
        0
    );
    let final_conn = open_sqlite_read_only_connection(&main_index).unwrap();
    assert_canonical_index_shape(&final_conn);
    assert_eq!(
        file_identity(&corpus.source, "source PST").unwrap(),
        source_before
    );

    drop(final_conn);
    fs::remove_dir_all(corpus.root).expect("synthetic promotion corpus should be removed");
}

#[test]
fn post_commit_promotion_failure_preserves_the_installed_completed_index() {
    let corpus = create_synthetic_import_corpus("post-commit-promotion");
    let source_before = file_identity(&corpus.source, "source PST").unwrap();
    let (temp_index, mut conn) = open_synthetic_index(&corpus);
    let stats = run_synthetic_index(&corpus, &mut conn, 4, |_| false).unwrap();
    apply_conversation_index(&mut conn, &corpus.identity.workspace_id, || false).unwrap();
    finalize_import_index_metadata(&mut conn, &stats).unwrap();
    set_metadata_value(&conn, "synthetic_promotion_generation", "replacement").unwrap();
    optimize_then_checked_truncate(&conn).unwrap();
    drop(conn);

    let discovery = discover_eml_candidates(&corpus.extract_dir).unwrap();
    fail_next_index_promotion_after_commit();
    let error = promote_completed_index(
        &corpus.workspace,
        &temp_index,
        &[corpus.identity.workspace_id.as_str()],
        &corpus.identity.content_fingerprint,
        &discovery,
    )
    .expect_err("the synthetic post-commit fault should be reported");
    assert_eq!(error.code, Some(INDEX_PROMOTION_STATE_UNCERTAIN_CODE));

    let main_index = corpus.workspace.join("index.sqlite");
    assert!(main_index.is_file());
    assert!(!temp_index.exists());
    let installed = open_sqlite_read_only_connection(&main_index).unwrap();
    assert_eq!(
        metadata_value(&installed, "synthetic_promotion_generation")
            .unwrap()
            .as_deref(),
        Some("replacement")
    );
    assert_eq!(
        metadata_value(&installed, "import_status")
            .unwrap()
            .as_deref(),
        Some("complete")
    );
    assert_canonical_index_shape(&installed);
    assert_eq!(
        file_identity(&corpus.source, "source PST").unwrap(),
        source_before
    );

    drop(installed);
    fs::remove_dir_all(corpus.root)
        .expect("synthetic post-commit promotion corpus should be removed");
}

#[test]
fn canonical_uniqueness_requires_one_nonpartial_binary_path_key() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE messages (eml_path TEXT NOT NULL, subject TEXT NOT NULL);
         CREATE UNIQUE INDEX partial_path ON messages(eml_path) WHERE subject <> '';",
    )
    .unwrap();
    assert!(!has_unique_message_source_identity(&conn).unwrap());

    conn.execute_batch(
        "DROP INDEX partial_path;
         CREATE UNIQUE INDEX nocase_path ON messages(eml_path COLLATE NOCASE);",
    )
    .unwrap();
    assert!(!has_unique_message_source_identity(&conn).unwrap());

    conn.execute_batch(
        "DROP INDEX nocase_path;
         CREATE UNIQUE INDEX expression_path ON messages(eml_path, lower(subject));",
    )
    .unwrap();
    assert!(!has_unique_message_source_identity(&conn).unwrap());

    conn.execute_batch(
        "DROP INDEX expression_path;
         CREATE UNIQUE INDEX exact_path ON messages(eml_path COLLATE BINARY);",
    )
    .unwrap();
    assert!(has_unique_message_source_identity(&conn).unwrap());
}

#[test]
fn malformed_nonempty_candidate_is_retained_as_a_parse_failure() {
    let root = temporary_import_integrity_directory("malformed-retained");
    let workspace = root.join("workspace");
    let extract_dir = workspace.join("extracted");
    create_private_directory_all(&workspace.join("logs")).unwrap();
    initialize_import_log(&workspace, "synthetic-readpst").unwrap();
    let malformed = extract_dir.join("Malformed/1.eml");
    write_synthetic_message(&malformed, b" invalid-header\r\n\r\nsynthetic payload\r\n");
    assert!(parse_eml(&malformed).is_err());

    let index_path = workspace.join("index.sqlite");
    let mut conn = open_sqlite_write_connection(&index_path).unwrap();
    initialize_schema(&conn).unwrap();
    let discovery = discover_eml_candidates(&extract_dir).unwrap();
    let stats = stream_index_eml_files_with_control(
        &workspace,
        &extract_dir,
        &mut conn,
        &discovery,
        1,
        "Synthetic import cancellation.",
        |_| false,
        |_| {},
    )
    .expect("malformed nonempty candidate should remain visible");
    assert_eq!(stats.error_count, 1);
    assert_eq!(stats.inserted, 1);
    assert_eq!(stats.indexed, 1);
    assert_eq!(stats.suppressed_empty_artifacts, 0);
    assert_eq!(
        scalar_count(
            &conn,
            "SELECT COUNT(*) FROM messages WHERE body_source = 'parse_error'",
        ),
        1
    );

    drop(conn);
    fs::remove_dir_all(root).expect("synthetic malformed corpus should be removed");
}

#[test]
fn blank_no_subject_and_empty_body_candidates_all_fail_open() {
    let corpus = create_synthetic_import_corpus("blank-fail-open");
    let (_index_path, mut conn) = open_synthetic_index(&corpus);
    let stats = run_synthetic_index(&corpus, &mut conn, 3, |_| false)
        .expect("all uncertain blank candidates should remain visible");

    assert_eq!(stats.discovered, 10);
    assert_eq!(stats.processed, 10);
    assert_eq!(stats.indexed, 10);
    assert_eq!(stats.inserted, 10);
    assert_eq!(stats.suppressed_empty_artifacts, 0);
    assert_canonical_index_shape(&conn);

    drop(conn);
    fs::remove_dir_all(corpus.root).expect("synthetic fail-open corpus should be removed");
}
