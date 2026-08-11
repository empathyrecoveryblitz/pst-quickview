use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{params, types::Value, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{AppError, AppResult};

const MAX_QUERY_SCALARS: usize = 512;
const MAX_PARSED_CLAUSES: usize = 32;
const MAX_TOKEN_SCALARS: usize = 64;
const MAX_PHRASE_SCALARS: usize = 256;
const MAX_STRUCTURED_FILTER_SCALARS: usize = 256;
const MAX_SEARCH_SNIPPET_SCALARS: usize = 240;
const MAX_SEARCH_HIGHLIGHT_RANGES: usize = 8;
// messages_fts columns: subject, sender, recipients, body, attachment_names.
pub(crate) const RELEVANCE_SCORE_SQL: &str = "bm25(messages_fts, 8.0, 4.0, 3.0, 1.0, 5.0)";
const RELEVANCE_SORT_SQL: &str =
    "bm25(messages_fts, 8.0, 4.0, 3.0, 1.0, 5.0) ASC, m.date DESC, m.id DESC";
const SEARCH_HIGHLIGHT_START: &str = "\u{1e}PSTQV-HIGHLIGHT-START\u{1f}";
const SEARCH_HIGHLIGHT_END: &str = "\u{1e}PSTQV-HIGHLIGHT-END\u{1f}";

const SEARCH_MATCH_SELECT_SQL: &str = ", snippet(messages_fts, -1,
                char(30) || 'PSTQV-HIGHLIGHT-START' || char(31),
                char(30) || 'PSTQV-HIGHLIGHT-END' || char(31),
                ' … ', 32),
       snippet(messages_fts, 0,
                char(30) || 'PSTQV-HIGHLIGHT-START' || char(31),
                char(30) || 'PSTQV-HIGHLIGHT-END' || char(31),
                ' … ', 32),
       snippet(messages_fts, 1,
                char(30) || 'PSTQV-HIGHLIGHT-START' || char(31),
                char(30) || 'PSTQV-HIGHLIGHT-END' || char(31),
                ' … ', 32),
       snippet(messages_fts, 2,
                char(30) || 'PSTQV-HIGHLIGHT-START' || char(31),
                char(30) || 'PSTQV-HIGHLIGHT-END' || char(31),
                ' … ', 32),
       snippet(messages_fts, 3,
                char(30) || 'PSTQV-HIGHLIGHT-START' || char(31),
                char(30) || 'PSTQV-HIGHLIGHT-END' || char(31),
                ' … ', 32),
       snippet(messages_fts, 4,
                char(30) || 'PSTQV-HIGHLIGHT-START' || char(31),
                char(30) || 'PSTQV-HIGHLIGHT-END' || char(31),
                ' … ', 32)";
const EMPTY_SEARCH_MATCH_SELECT_SQL: &str = ", NULL, NULL, NULL, NULL, NULL, NULL";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchHighlightRange {
    // Serialized offsets use UTF-16 code units, matching JavaScript String.slice.
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SearchMatchedField {
    Subject,
    Sender,
    Recipients,
    Body,
    Attachment,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchMatchContext {
    pub(crate) snippet_text: String,
    pub(crate) highlight_ranges: Vec<SearchHighlightRange>,
    pub(crate) matched_fields: Vec<SearchMatchedField>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchFilters {
    from: Option<String>,
    recipients: Option<String>,
    subject: Option<String>,
    body: Option<String>,
    attachment: Option<String>,
    has_attachments: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchValidationError {
    QueryTooLong,
    TooManyClauses,
    TokenTooLong,
    PhraseTooLong,
    StructuredFilterTooLong { field: &'static str },
    InvalidDate { field: &'static str },
    ReversedDateRange,
    RelevanceRequiresText,
    RelevanceRequiresSingleWorkspace,
    RelevanceUnsupportedForConversations,
}

impl std::fmt::Display for SearchValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueryTooLong => write!(
                formatter,
                "Search query exceeds the {MAX_QUERY_SCALARS}-character limit."
            ),
            Self::TooManyClauses => write!(
                formatter,
                "Search query exceeds the {MAX_PARSED_CLAUSES}-clause limit."
            ),
            Self::TokenTooLong => write!(
                formatter,
                "Search token exceeds the {MAX_TOKEN_SCALARS}-character limit."
            ),
            Self::PhraseTooLong => write!(
                formatter,
                "Search phrase exceeds the {MAX_PHRASE_SCALARS}-character limit."
            ),
            Self::StructuredFilterTooLong { field } => write!(
                formatter,
                "Search {field} filter exceeds the {MAX_STRUCTURED_FILTER_SCALARS}-character limit."
            ),
            Self::InvalidDate { field } => write!(
                formatter,
                "Search {field} must be a valid YYYY-MM-DD date or RFC 3339 timestamp."
            ),
            Self::ReversedDateRange => write!(
                formatter,
                "Search date range is reversed; date_from must not be after date_to."
            ),
            Self::RelevanceRequiresText => {
                write!(formatter, "Relevance sorting requires a text search.")
            }
            Self::RelevanceRequiresSingleWorkspace => write!(
                formatter,
                "Relevance sorting is available only when searching one PST workspace."
            ),
            Self::RelevanceUnsupportedForConversations => write!(
                formatter,
                "Relevance sorting is not available for Conversations."
            ),
        }
    }
}

impl std::error::Error for SearchValidationError {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchToken {
    value: String,
    quoted: bool,
}

#[derive(Clone, Debug)]
struct SearchDateBound {
    sql_value: String,
    instant: DateTime<Utc>,
    inclusive: bool,
}

#[derive(Debug, Default)]
pub(crate) struct MessageSearchCriteria {
    fts_terms: Vec<SearchToken>,
    from_terms: Vec<SearchToken>,
    recipient_terms: Vec<SearchToken>,
    subject_terms: Vec<SearchToken>,
    body_terms: Vec<SearchToken>,
    attachment_terms: Vec<SearchToken>,
    has_attachments: Option<bool>,
    date_from: Option<SearchDateBound>,
    date_to: Option<SearchDateBound>,
    clause_count: usize,
}

impl MessageSearchCriteria {
    pub(crate) fn from_inputs(
        query: Option<String>,
        filters: Option<SearchFilters>,
    ) -> Result<Self, SearchValidationError> {
        let mut criteria = Self::default();
        if let Some(query) = query {
            criteria.apply_query_syntax(&query)?;
        }
        if let Some(filters) = filters {
            criteria.apply_filters(filters)?;
        }
        criteria.validate_date_range()?;
        Ok(criteria)
    }

    fn apply_filters(&mut self, filters: SearchFilters) -> Result<(), SearchValidationError> {
        self.apply_structured_text_filter(SearchField::From, filters.from)?;
        self.apply_structured_text_filter(SearchField::Recipients, filters.recipients)?;
        self.apply_structured_text_filter(SearchField::Subject, filters.subject)?;
        self.apply_structured_text_filter(SearchField::Body, filters.body)?;
        self.apply_structured_text_filter(SearchField::Attachment, filters.attachment)?;

        if let Some(value) = clean_filter_value(filters.has_attachments) {
            validate_unquoted_value(&value)?;
            self.add_clauses(1)?;
            self.has_attachments = parse_has_attachments(&value).or(self.has_attachments);
        }
        if let Some(value) = clean_filter_value(filters.date_from) {
            validate_unquoted_value(&value)?;
            self.add_clauses(1)?;
            self.date_from = Some(parse_date_bound(&value, "date_from", false)?);
        }
        if let Some(value) = clean_filter_value(filters.date_to) {
            validate_unquoted_value(&value)?;
            self.add_clauses(1)?;
            self.date_to = Some(parse_date_bound(&value, "date_to", true)?);
        }
        Ok(())
    }

    fn apply_structured_text_filter(
        &mut self,
        field: SearchField,
        value: Option<String>,
    ) -> Result<(), SearchValidationError> {
        let Some(value) = clean_filter_value(value) else {
            return Ok(());
        };
        if scalar_count(&value) > MAX_STRUCTURED_FILTER_SCALARS {
            return Err(SearchValidationError::StructuredFilterTooLong {
                field: field.filter_name(),
            });
        }

        let tokens = tokenize_search_query(&value);
        for token in &tokens {
            validate_token(token)?;
        }
        self.add_clauses(tokens.len().max(1))?;
        self.push_field_tokens(field, tokens);
        Ok(())
    }

    fn apply_query_syntax(&mut self, query: &str) -> Result<(), SearchValidationError> {
        if scalar_count(query) > MAX_QUERY_SCALARS {
            return Err(SearchValidationError::QueryTooLong);
        }

        let tokens = tokenize_search_query(query);
        let mut index = 0;
        while index < tokens.len() {
            let token = &tokens[index];
            if !token.quoted {
                if let Some(key) = token.value.strip_suffix(':') {
                    if let Some(next) = tokens.get(index + 1) {
                        validate_field_name(key)?;
                        validate_token(next)?;
                        self.add_clauses(1)?;
                        self.apply_typed_token(key, next.clone())?;
                        index += 2;
                        continue;
                    }
                }

                if let Some((key, value)) = split_typed_token(&token.value) {
                    let value_token = SearchToken {
                        value: value.to_string(),
                        quoted: false,
                    };
                    validate_field_name(key)?;
                    if normalize_field_name(key).is_some() {
                        validate_token(&value_token)?;
                    } else {
                        validate_token(token)?;
                    }
                    self.add_clauses(1)?;
                    self.apply_typed_token(key, value_token)?;
                    index += 1;
                    continue;
                }
            }

            validate_token(token)?;
            self.add_clauses(1)?;
            if !token.value.trim().is_empty() {
                self.fts_terms.push(token.clone());
            }
            index += 1;
        }
        Ok(())
    }

    fn apply_typed_token(
        &mut self,
        key: &str,
        token: SearchToken,
    ) -> Result<(), SearchValidationError> {
        let value = token.value.trim();
        if value.is_empty() {
            return Ok(());
        }

        match normalize_field_name(key) {
            Some(SearchField::From) => self.from_terms.push(token),
            Some(SearchField::Recipients) => self.recipient_terms.push(token),
            Some(SearchField::Subject) => self.subject_terms.push(token),
            Some(SearchField::Body) => self.body_terms.push(token),
            Some(SearchField::Attachment) => self.attachment_terms.push(token),
            Some(SearchField::HasAttachments) => {
                if let Some(has_attachments) = parse_has_attachments(value) {
                    self.has_attachments = Some(has_attachments);
                }
            }
            Some(SearchField::DateFrom) => {
                self.date_from = Some(parse_date_bound(value, "date_from", false)?);
            }
            Some(SearchField::DateTo) => {
                self.date_to = Some(parse_date_bound(value, "date_to", true)?);
            }
            None => {
                let unknown = SearchToken {
                    value: format!("{key}:{value}"),
                    quoted: false,
                };
                validate_token(&unknown)?;
                self.fts_terms.push(unknown);
            }
        }
        Ok(())
    }

    fn push_field_tokens(&mut self, field: SearchField, tokens: Vec<SearchToken>) {
        match field {
            SearchField::From => self.from_terms.extend(tokens),
            SearchField::Recipients => self.recipient_terms.extend(tokens),
            SearchField::Subject => self.subject_terms.extend(tokens),
            SearchField::Body => self.body_terms.extend(tokens),
            SearchField::Attachment => self.attachment_terms.extend(tokens),
            SearchField::HasAttachments | SearchField::DateFrom | SearchField::DateTo => {}
        }
    }

    fn add_clauses(&mut self, count: usize) -> Result<(), SearchValidationError> {
        self.clause_count = self.clause_count.saturating_add(count);
        if self.clause_count > MAX_PARSED_CLAUSES {
            return Err(SearchValidationError::TooManyClauses);
        }
        Ok(())
    }

    fn validate_date_range(&self) -> Result<(), SearchValidationError> {
        let (Some(start), Some(end)) = (&self.date_from, &self.date_to) else {
            return Ok(());
        };
        let reversed = if end.inclusive {
            start.instant > end.instant
        } else {
            start.instant >= end.instant
        };
        if reversed {
            return Err(SearchValidationError::ReversedDateRange);
        }
        Ok(())
    }

    pub(crate) fn cursor_fingerprint(
        &self,
        folder_id: Option<i64>,
        include_subfolders: bool,
    ) -> [u8; 16] {
        let mut hasher = Sha256::new();
        hasher.update(b"pst-quickview-message-criteria-v1");
        hash_search_tokens(&mut hasher, b"any", &self.fts_terms);
        hash_search_tokens(&mut hasher, b"from", &self.from_terms);
        hash_search_tokens(&mut hasher, b"recipients", &self.recipient_terms);
        hash_search_tokens(&mut hasher, b"subject", &self.subject_terms);
        hash_search_tokens(&mut hasher, b"body", &self.body_terms);
        hash_search_tokens(&mut hasher, b"attachment", &self.attachment_terms);
        hasher.update([match self.has_attachments {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        }]);
        hash_date_bound(&mut hasher, b"date-from", self.date_from.as_ref());
        hash_date_bound(&mut hasher, b"date-to", self.date_to.as_ref());
        match folder_id {
            Some(id) => {
                hasher.update([1]);
                hasher.update(id.to_be_bytes());
            }
            None => hasher.update([0]),
        }
        hasher.update([u8::from(include_subfolders)]);
        let digest = hasher.finalize();
        let mut fingerprint = [0u8; 16];
        fingerprint.copy_from_slice(&digest[..16]);
        fingerprint
    }
}

fn hash_search_tokens(hasher: &mut Sha256, label: &[u8], tokens: &[SearchToken]) {
    hasher.update((label.len() as u64).to_be_bytes());
    hasher.update(label);
    hasher.update((tokens.len() as u64).to_be_bytes());
    for token in tokens {
        hasher.update([u8::from(token.quoted)]);
        hasher.update((token.value.len() as u64).to_be_bytes());
        hasher.update(token.value.as_bytes());
    }
}

fn hash_date_bound(hasher: &mut Sha256, label: &[u8], bound: Option<&SearchDateBound>) {
    hasher.update((label.len() as u64).to_be_bytes());
    hasher.update(label);
    if let Some(bound) = bound {
        hasher.update([1, u8::from(bound.inclusive)]);
        hasher.update((bound.sql_value.len() as u64).to_be_bytes());
        hasher.update(bound.sql_value.as_bytes());
    } else {
        hasher.update([0]);
    }
}

#[derive(Clone, Copy)]
enum SearchField {
    From,
    Recipients,
    Subject,
    Body,
    Attachment,
    HasAttachments,
    DateFrom,
    DateTo,
}

impl SearchField {
    fn filter_name(self) -> &'static str {
        match self {
            Self::From => "from",
            Self::Recipients => "recipients",
            Self::Subject => "subject",
            Self::Body => "body",
            Self::Attachment => "attachment",
            Self::HasAttachments => "has_attachments",
            Self::DateFrom => "date_from",
            Self::DateTo => "date_to",
        }
    }
}

fn normalize_field_name(key: &str) -> Option<SearchField> {
    match key.to_ascii_lowercase().as_str() {
        "from" => Some(SearchField::From),
        "to" | "cc" | "bcc" | "recipient" | "recipients" => Some(SearchField::Recipients),
        "subject" | "subj" => Some(SearchField::Subject),
        "body" | "text" => Some(SearchField::Body),
        "attachment" | "attach" | "filename" => Some(SearchField::Attachment),
        "has" => Some(SearchField::HasAttachments),
        "after" => Some(SearchField::DateFrom),
        "before" => Some(SearchField::DateTo),
        _ => None,
    }
}

fn clean_filter_value(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_has_attachments(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" | "attachment" | "attachments" => Some(true),
        "no" | "false" | "0" | "none" | "noattachment" | "noattachments" | "no-attachment"
        | "no-attachments" => Some(false),
        _ => None,
    }
}

fn parse_date_bound(
    value: &str,
    field: &'static str,
    end_of_date: bool,
) -> Result<SearchDateBound, SearchValidationError> {
    let value = value.trim();
    if is_date_only_shape(value) {
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .map_err(|_| SearchValidationError::InvalidDate { field })?;
        let bound_date = if end_of_date {
            date.succ_opt()
                .ok_or(SearchValidationError::InvalidDate { field })?
        } else {
            date
        };
        let instant = bound_date
            .and_hms_opt(0, 0, 0)
            .expect("midnight is a valid time")
            .and_utc();
        return Ok(SearchDateBound {
            sql_value: instant.to_rfc3339(),
            instant,
            inclusive: !end_of_date,
        });
    }

    let instant = DateTime::parse_from_rfc3339(value)
        .map_err(|_| SearchValidationError::InvalidDate { field })?
        .with_timezone(&Utc);
    Ok(SearchDateBound {
        sql_value: instant.to_rfc3339(),
        instant,
        inclusive: true,
    })
}

fn is_date_only_shape(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
}

fn scalar_count(value: &str) -> usize {
    value.chars().count()
}

fn validate_field_name(value: &str) -> Result<(), SearchValidationError> {
    if scalar_count(value) > MAX_TOKEN_SCALARS {
        return Err(SearchValidationError::TokenTooLong);
    }
    Ok(())
}

fn validate_unquoted_value(value: &str) -> Result<(), SearchValidationError> {
    validate_token(&SearchToken {
        value: value.to_string(),
        quoted: false,
    })
}

fn validate_token(token: &SearchToken) -> Result<(), SearchValidationError> {
    let length = scalar_count(token.value.trim());
    if token.quoted {
        if length > MAX_PHRASE_SCALARS {
            return Err(SearchValidationError::PhraseTooLong);
        }
    } else if length > MAX_TOKEN_SCALARS {
        return Err(SearchValidationError::TokenTooLong);
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct MessageQuerySource {
    pub(crate) from_sql: String,
    pub(crate) where_sql: String,
    pub(crate) params: Vec<Value>,
    pub(crate) has_text_match: bool,
}

pub(crate) fn build_message_query_source(
    conn: &Connection,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
) -> AppResult<MessageQuerySource> {
    let fts_query = build_combined_fts_query(criteria);
    let has_text_match = fts_query.is_some();
    let folder_scope = folder_scope(conn, folder_id, include_subfolders)?;
    let mut from_sql = String::from(" FROM messages m LEFT JOIN folders f ON f.id = m.folder_id ");
    let mut conditions = Vec::new();
    let mut query_params = Vec::<Value>::new();

    if fts_query.is_some() {
        from_sql.push_str("JOIN messages_fts ON messages_fts.rowid = m.id ");
    }

    if matches!(folder_scope, FolderScope::WithDescendants { .. }) {
        from_sql.push_str("JOIN folders folder_scope ON folder_scope.id = m.folder_id ");
    }

    if let Some(fts_query) = fts_query {
        conditions.push("messages_fts MATCH ?".to_string());
        query_params.push(Value::Text(fts_query));
    }

    match folder_scope {
        FolderScope::All => {}
        FolderScope::Exact(id) => {
            conditions.push("m.folder_id = ?".to_string());
            query_params.push(Value::Integer(id));
        }
        FolderScope::WithDescendants { id, prefix } => {
            conditions
                .push("(folder_scope.id = ? OR folder_scope.path LIKE ? ESCAPE '\\')".to_string());
            query_params.push(Value::Integer(id));
            query_params.push(Value::Text(prefix));
        }
    }

    if let Some(has_attachments) = criteria.has_attachments {
        if has_attachments {
            conditions.push(
                "(m.has_attachments != 0
                  OR EXISTS (SELECT 1 FROM attachments attachment_presence WHERE attachment_presence.message_id = m.id))"
                    .to_string(),
            );
        } else {
            conditions.push(
                "(m.has_attachments = 0
                  AND NOT EXISTS (SELECT 1 FROM attachments attachment_presence WHERE attachment_presence.message_id = m.id))"
                    .to_string(),
            );
        }
    }

    if criteria.date_from.is_some() || criteria.date_to.is_some() {
        conditions.push("m.date IS NOT NULL AND m.date <> ''".to_string());
    }
    if let Some(date_from) = &criteria.date_from {
        conditions.push("m.date >= ?".to_string());
        query_params.push(Value::Text(date_from.sql_value.clone()));
    }
    if let Some(date_to) = &criteria.date_to {
        conditions.push(if date_to.inclusive {
            "m.date <= ?".to_string()
        } else {
            "m.date < ?".to_string()
        });
        query_params.push(Value::Text(date_to.sql_value.clone()));
    }

    let where_sql = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {} ", conditions.join(" AND "))
    };

    Ok(MessageQuerySource {
        from_sql,
        where_sql,
        params: query_params,
        has_text_match,
    })
}

pub(crate) fn search_match_select_sql(has_text_match: bool) -> &'static str {
    if has_text_match {
        SEARCH_MATCH_SELECT_SQL
    } else {
        EMPTY_SEARCH_MATCH_SELECT_SQL
    }
}

pub(crate) fn search_match_context_from_row(
    row: &Row<'_>,
    start_index: usize,
) -> rusqlite::Result<Option<SearchMatchContext>> {
    let best = row.get::<_, Option<String>>(start_index)?;
    let fields = [
        (
            SearchMatchedField::Subject,
            row.get::<_, Option<String>>(start_index + 1)?,
        ),
        (
            SearchMatchedField::Sender,
            row.get::<_, Option<String>>(start_index + 2)?,
        ),
        (
            SearchMatchedField::Recipients,
            row.get::<_, Option<String>>(start_index + 3)?,
        ),
        (
            SearchMatchedField::Body,
            row.get::<_, Option<String>>(start_index + 4)?,
        ),
        (
            SearchMatchedField::Attachment,
            row.get::<_, Option<String>>(start_index + 5)?,
        ),
    ];
    Ok(build_search_match_context(best.as_deref(), &fields))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MarkedSnippet {
    text: String,
    highlight_ranges: Vec<SearchHighlightRange>,
    has_highlight: bool,
}

fn build_search_match_context(
    best: Option<&str>,
    field_values: &[(SearchMatchedField, Option<String>)],
) -> Option<SearchMatchContext> {
    let parsed_fields = field_values
        .iter()
        .filter_map(|(field, value)| {
            let parsed = parse_marked_snippet(value.as_deref()?)?;
            parsed.has_highlight.then_some((*field, parsed))
        })
        .collect::<Vec<_>>();
    let matched_fields = parsed_fields
        .iter()
        .map(|(field, _)| *field)
        .take(5)
        .collect::<Vec<_>>();
    if matched_fields.is_empty() {
        return None;
    }

    let selected = best
        .and_then(parse_marked_snippet)
        .filter(|snippet| snippet.has_highlight)
        .or_else(|| parsed_fields.first().map(|(_, snippet)| snippet.clone()))?;
    if selected.text.is_empty() || selected.highlight_ranges.is_empty() {
        return None;
    }

    Some(SearchMatchContext {
        snippet_text: selected.text,
        highlight_ranges: selected.highlight_ranges,
        matched_fields,
    })
}

fn parse_marked_snippet(value: &str) -> Option<MarkedSnippet> {
    let mut parsed = Vec::<(char, bool)>::new();
    let mut cursor = 0usize;
    let mut highlighted = false;
    let mut saw_highlight = false;

    while cursor < value.len() {
        let remaining = &value[cursor..];
        if remaining.starts_with(SEARCH_HIGHLIGHT_START) {
            if highlighted {
                return None;
            }
            highlighted = true;
            saw_highlight = true;
            cursor += SEARCH_HIGHLIGHT_START.len();
            continue;
        }
        if remaining.starts_with(SEARCH_HIGHLIGHT_END) {
            if !highlighted {
                return None;
            }
            highlighted = false;
            cursor += SEARCH_HIGHLIGHT_END.len();
            continue;
        }

        let character = remaining.chars().next()?;
        parsed.push((character, highlighted));
        cursor += character.len_utf8();
    }
    if highlighted {
        return None;
    }

    let normalized = normalize_marked_whitespace(parsed);
    let bounded = bound_marked_snippet(normalized, MAX_SEARCH_SNIPPET_SCALARS);
    let (text, highlight_ranges) = marked_chars_to_text_and_ranges(&bounded);
    Some(MarkedSnippet {
        text,
        has_highlight: saw_highlight && !highlight_ranges.is_empty(),
        highlight_ranges,
    })
}

fn normalize_marked_whitespace(value: Vec<(char, bool)>) -> Vec<(char, bool)> {
    let mut normalized = Vec::with_capacity(value.len());
    let mut pending_space = false;
    let mut pending_space_highlight = false;

    for (character, highlighted) in value {
        if character.is_whitespace() {
            if !normalized.is_empty() {
                pending_space = true;
                pending_space_highlight |= highlighted;
            }
            continue;
        }
        if pending_space {
            normalized.push((' ', pending_space_highlight));
            pending_space = false;
            pending_space_highlight = false;
        }
        normalized.push((character, highlighted));
    }
    normalized
}

fn bound_marked_snippet(value: Vec<(char, bool)>, maximum_scalars: usize) -> Vec<(char, bool)> {
    if value.len() <= maximum_scalars || maximum_scalars < 3 {
        return value;
    }

    let first_highlight = value
        .iter()
        .position(|(_, highlighted)| *highlighted)
        .unwrap_or(0);
    let content_limit = maximum_scalars - 2;
    let mut start = first_highlight.saturating_sub(content_limit / 2);
    if start + content_limit > value.len() {
        start = value.len().saturating_sub(content_limit);
    }
    let end = (start + content_limit).min(value.len());
    let mut bounded = Vec::with_capacity(maximum_scalars);
    if start > 0 {
        bounded.push(('…', false));
    }
    bounded.extend_from_slice(&value[start..end]);
    if end < value.len() {
        bounded.push(('…', false));
    }
    bounded
}

fn marked_chars_to_text_and_ranges(value: &[(char, bool)]) -> (String, Vec<SearchHighlightRange>) {
    let mut text = String::new();
    let mut ranges = Vec::new();
    let mut utf16_offset = 0usize;
    let mut range_start = None;

    for (character, highlighted) in value {
        if *highlighted && range_start.is_none() && ranges.len() < MAX_SEARCH_HIGHLIGHT_RANGES {
            range_start = Some(utf16_offset);
        } else if !*highlighted {
            if let Some(start) = range_start.take() {
                ranges.push(SearchHighlightRange {
                    start,
                    end: utf16_offset,
                });
            }
        }
        text.push(*character);
        utf16_offset += character.len_utf16();
    }
    if let Some(start) = range_start {
        if ranges.len() < MAX_SEARCH_HIGHLIGHT_RANGES {
            ranges.push(SearchHighlightRange {
                start,
                end: utf16_offset,
            });
        }
    }
    (text, ranges)
}

pub(crate) fn query_source_with_condition(
    source: &MessageQuerySource,
    condition: &str,
) -> (String, Vec<Value>) {
    let where_sql = if source.where_sql.trim().is_empty() {
        format!(" WHERE {condition} ")
    } else {
        let trimmed = source.where_sql.trim();
        let existing = trimmed.strip_prefix("WHERE").unwrap_or(trimmed).trim();
        format!(" WHERE ({existing}) AND {condition} ")
    };
    (where_sql, source.params.clone())
}

pub(crate) fn validate_message_sort_workspace_count(
    sort_order: Option<&str>,
    workspace_count: usize,
) -> Result<(), SearchValidationError> {
    if sort_order == Some("relevance") && workspace_count != 1 {
        return Err(SearchValidationError::RelevanceRequiresSingleWorkspace);
    }
    Ok(())
}

pub(crate) fn validate_conversation_sort(sort: &str) -> Result<(), SearchValidationError> {
    if sort == "relevance" {
        return Err(SearchValidationError::RelevanceUnsupportedForConversations);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageSort {
    Newest,
    Oldest,
    Sender,
    Subject,
    Relevance,
}

impl MessageSort {
    pub(crate) fn from_request(
        sort_order: Option<&str>,
        has_text_match: bool,
    ) -> Result<Self, SearchValidationError> {
        match sort_order.unwrap_or("newest") {
            "relevance" if has_text_match => Ok(Self::Relevance),
            "relevance" => Err(SearchValidationError::RelevanceRequiresText),
            "oldest" => Ok(Self::Oldest),
            "sender_az" => Ok(Self::Sender),
            "subject_az" => Ok(Self::Subject),
            _ => Ok(Self::Newest),
        }
    }

    pub(crate) fn sql(self) -> &'static str {
        match self {
            Self::Newest => "m.date DESC, m.id DESC",
            Self::Oldest => "m.date ASC, m.id ASC",
            Self::Sender => "m.sender COLLATE NOCASE ASC, m.date DESC, m.id DESC",
            Self::Subject => "m.subject COLLATE NOCASE ASC, m.date DESC, m.id DESC",
            Self::Relevance => RELEVANCE_SORT_SQL,
        }
    }

    pub(crate) fn cursor_tag(self) -> u8 {
        match self {
            Self::Newest => 0,
            Self::Oldest => 1,
            Self::Sender => 2,
            Self::Subject => 3,
            Self::Relevance => 4,
        }
    }

    pub(crate) fn from_cursor_tag(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Newest),
            1 => Some(Self::Oldest),
            2 => Some(Self::Sender),
            3 => Some(Self::Subject),
            4 => Some(Self::Relevance),
            _ => None,
        }
    }
}

pub(crate) fn message_sort_clause(
    sort_order: Option<&str>,
    has_text_match: bool,
) -> Result<&'static str, SearchValidationError> {
    Ok(MessageSort::from_request(sort_order, has_text_match)?.sql())
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MessageKeysetBoundary {
    Newest {
        date: Option<String>,
        id: i64,
    },
    Oldest {
        date: Option<String>,
        id: i64,
    },
    Sender {
        sender: Option<String>,
        date: Option<String>,
        id: i64,
    },
    Subject {
        subject: Option<String>,
        date: Option<String>,
        id: i64,
    },
    Relevance {
        score: f64,
        date: Option<String>,
        id: i64,
    },
}

pub(crate) fn resolve_message_keyset_boundary(
    conn: &Connection,
    source: &MessageQuerySource,
    sort: MessageSort,
    message_id: i64,
) -> rusqlite::Result<Option<MessageKeysetBoundary>> {
    let (where_sql, mut params) = query_source_with_condition(source, "m.id = ?");
    params.push(Value::Integer(message_id));
    let select_sql = match sort {
        MessageSort::Newest | MessageSort::Oldest => "m.date, NULL, NULL",
        MessageSort::Sender => "m.date, m.sender, NULL",
        MessageSort::Subject => "m.date, NULL, m.subject",
        MessageSort::Relevance => "m.date, NULL, NULL, bm25(messages_fts, 8.0, 4.0, 3.0, 1.0, 5.0)",
    };
    let sql = format!(
        "SELECT {select_sql}{}{} LIMIT 1",
        source.from_sql, where_sql
    );
    conn.query_row(&sql, rusqlite::params_from_iter(params.iter()), |row| {
        let date = row.get::<_, Option<String>>(0)?;
        Ok(match sort {
            MessageSort::Newest => MessageKeysetBoundary::Newest {
                date,
                id: message_id,
            },
            MessageSort::Oldest => MessageKeysetBoundary::Oldest {
                date,
                id: message_id,
            },
            MessageSort::Sender => MessageKeysetBoundary::Sender {
                sender: row.get(1)?,
                date,
                id: message_id,
            },
            MessageSort::Subject => MessageKeysetBoundary::Subject {
                subject: row.get(2)?,
                date,
                id: message_id,
            },
            MessageSort::Relevance => MessageKeysetBoundary::Relevance {
                score: row.get(3)?,
                date,
                id: message_id,
            },
        })
    })
    .optional()
}

pub(crate) fn message_keyset_condition(boundary: &MessageKeysetBoundary) -> (String, Vec<Value>) {
    match boundary {
        MessageKeysetBoundary::Newest { date, id } => descending_date_condition(date, *id),
        MessageKeysetBoundary::Oldest { date, id } => ascending_date_condition(date, *id),
        MessageKeysetBoundary::Sender { sender, date, id } => {
            let (date_sql, date_params) = descending_date_condition(date, *id);
            if let Some(sender) = sender {
                let mut params = vec![Value::Text(sender.clone()), Value::Text(sender.clone())];
                params.extend(date_params);
                (
                    format!(
                        "(m.sender COLLATE NOCASE > ? OR
                          (m.sender COLLATE NOCASE = ? AND ({date_sql})))"
                    ),
                    params,
                )
            } else {
                (
                    format!("(m.sender IS NOT NULL OR (m.sender IS NULL AND ({date_sql})))"),
                    date_params,
                )
            }
        }
        MessageKeysetBoundary::Subject { subject, date, id } => {
            let (date_sql, date_params) = descending_date_condition(date, *id);
            if let Some(subject) = subject {
                let mut params = vec![Value::Text(subject.clone()), Value::Text(subject.clone())];
                params.extend(date_params);
                (
                    format!(
                        "(m.subject COLLATE NOCASE > ? OR
                          (m.subject COLLATE NOCASE = ? AND ({date_sql})))"
                    ),
                    params,
                )
            } else {
                (
                    format!("(m.subject IS NOT NULL OR (m.subject IS NULL AND ({date_sql})))"),
                    date_params,
                )
            }
        }
        MessageKeysetBoundary::Relevance { score, date, id } => {
            let (date_sql, date_params) = descending_date_condition(date, *id);
            let mut params = vec![Value::Real(*score), Value::Real(*score)];
            params.extend(date_params);
            (
                format!(
                    "({RELEVANCE_SCORE_SQL} > ? OR
                      ({RELEVANCE_SCORE_SQL} = ? AND ({date_sql})))"
                ),
                params,
            )
        }
    }
}

fn descending_date_condition(date: &Option<String>, id: i64) -> (String, Vec<Value>) {
    if let Some(date) = date {
        (
            "(m.date < ? OR m.date IS NULL OR (m.date = ? AND m.id < ?))".to_string(),
            vec![
                Value::Text(date.clone()),
                Value::Text(date.clone()),
                Value::Integer(id),
            ],
        )
    } else {
        (
            "(m.date IS NULL AND m.id < ?)".to_string(),
            vec![Value::Integer(id)],
        )
    }
}

fn ascending_date_condition(date: &Option<String>, id: i64) -> (String, Vec<Value>) {
    if let Some(date) = date {
        (
            "(m.date > ? OR (m.date = ? AND m.id > ?))".to_string(),
            vec![
                Value::Text(date.clone()),
                Value::Text(date.clone()),
                Value::Integer(id),
            ],
        )
    } else {
        (
            "(m.date IS NOT NULL OR (m.date IS NULL AND m.id > ?))".to_string(),
            vec![Value::Integer(id)],
        )
    }
}

pub(crate) fn conversation_sort_clause(sort: &str) -> &'static str {
    match sort {
        "oldest" => "matched.date ASC, matched.id ASC",
        "subject" => "conversation_subject COLLATE NOCASE ASC, matched.date DESC, matched.id DESC",
        _ => "matched.date DESC, matched.id DESC",
    }
}

fn build_combined_fts_query(criteria: &MessageSearchCriteria) -> Option<String> {
    let mut terms = build_fts_terms(&criteria.fts_terms);
    append_column_fts_terms(&mut terms, "sender", &criteria.from_terms);
    append_column_fts_terms(&mut terms, "recipients", &criteria.recipient_terms);
    append_column_fts_terms(&mut terms, "subject", &criteria.subject_terms);
    append_column_fts_terms(&mut terms, "body", &criteria.body_terms);
    append_column_fts_terms(&mut terms, "attachment_names", &criteria.attachment_terms);

    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" AND "))
    }
}

fn append_column_fts_terms(terms: &mut Vec<String>, column: &str, tokens: &[SearchToken]) {
    terms.extend(
        build_fts_terms(tokens)
            .into_iter()
            .map(|term| format!("{column}:{term}")),
    );
}

fn build_fts_terms(tokens: &[SearchToken]) -> Vec<String> {
    let mut terms = Vec::new();
    for token in tokens {
        let words = token
            .value
            .split(|character: char| !character.is_alphanumeric())
            .map(str::trim)
            .filter(|term| !term.is_empty())
            .map(|term| term.to_ascii_lowercase())
            .collect::<Vec<_>>();

        if words.is_empty() {
            continue;
        }

        if token.quoted && words.len() > 1 {
            terms.push(format!("\"{}\"", words.join(" ")));
        } else {
            terms.extend(words.into_iter().map(|term| format!("{term}*")));
        }
    }
    terms
}

#[derive(Debug)]
enum FolderScope {
    All,
    Exact(i64),
    WithDescendants { id: i64, prefix: String },
}

fn folder_scope(
    conn: &Connection,
    folder_id: Option<i64>,
    include_subfolders: bool,
) -> AppResult<FolderScope> {
    let Some(id) = folder_id else {
        return Ok(FolderScope::All);
    };
    if !include_subfolders {
        return Ok(FolderScope::Exact(id));
    }

    let path = folder_path(conn, id)?;
    if path.is_empty() {
        return Ok(FolderScope::All);
    }
    Ok(FolderScope::WithDescendants {
        id,
        prefix: format!("{}/%", escape_like(&path)),
    })
}

fn folder_path(conn: &Connection, folder_id: i64) -> AppResult<String> {
    conn.query_row(
        "SELECT path FROM folders WHERE id = ?1",
        params![folder_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| AppError::new("Folder was not found in this workspace."))
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn tokenize_search_query(query: &str) -> Vec<SearchToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut current_quoted = false;

    for character in query.chars() {
        match character {
            '"' => {
                if in_quote {
                    let value = current.trim();
                    if !value.is_empty() {
                        tokens.push(SearchToken {
                            value: value.to_string(),
                            quoted: true,
                        });
                    }
                    current.clear();
                    in_quote = false;
                    current_quoted = false;
                } else {
                    if !current.trim().is_empty() {
                        tokens.push(SearchToken {
                            value: current.trim().to_string(),
                            quoted: current_quoted,
                        });
                    }
                    current.clear();
                    in_quote = true;
                    current_quoted = true;
                }
            }
            character if character.is_whitespace() && !in_quote => {
                let value = current.trim();
                if !value.is_empty() {
                    tokens.push(SearchToken {
                        value: value.to_string(),
                        quoted: current_quoted,
                    });
                }
                current.clear();
                current_quoted = false;
            }
            character => current.push(character),
        }
    }

    let value = current.trim();
    if !value.is_empty() {
        tokens.push(SearchToken {
            value: value.to_string(),
            quoted: current_quoted,
        });
    }
    tokens
}

fn split_typed_token(token: &str) -> Option<(&str, &str)> {
    let (key, value) = token.split_once(':')?;
    if key.is_empty()
        || value.is_empty()
        || !key.chars().all(|character| character.is_ascii_alphabetic())
    {
        return None;
    }
    Some((key, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params_from_iter;

    fn criteria(query: &str) -> MessageSearchCriteria {
        MessageSearchCriteria::from_inputs(Some(query.to_string()), None)
            .expect("query should be valid")
    }

    fn fts_query(query: &str) -> Option<String> {
        build_combined_fts_query(&criteria(query))
    }

    fn fixture_connection() -> Connection {
        let conn = Connection::open_in_memory().expect("test database should open");
        conn.execute_batch(
            "CREATE TABLE folders (
                 id INTEGER PRIMARY KEY,
                 parent_id INTEGER,
                 path TEXT NOT NULL,
                 name TEXT NOT NULL
             );
             CREATE TABLE messages (
                 id INTEGER PRIMARY KEY,
                 folder_id INTEGER NOT NULL,
                 subject TEXT,
                 sender TEXT,
                 recipients TEXT,
                 date TEXT,
                 body TEXT,
                 snippet TEXT,
                 attachment_names TEXT,
                 has_attachments INTEGER NOT NULL DEFAULT 0,
                 conversation_id TEXT
             );
             CREATE TABLE attachments (
                 id INTEGER PRIMARY KEY,
                 message_id INTEGER NOT NULL,
                 filename TEXT,
                 sanitized_filename TEXT,
                 content_type TEXT
             );
             CREATE VIRTUAL TABLE messages_fts
             USING fts5(subject, sender, recipients, body, attachment_names);",
        )
        .expect("search fixture schema should initialize");
        conn
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_message(
        conn: &Connection,
        id: i64,
        folder_id: i64,
        subject: &str,
        sender: &str,
        recipients: &str,
        date: &str,
        body: &str,
        attachment_names: &str,
        has_attachments: bool,
    ) {
        conn.execute(
            "INSERT INTO messages (
                 id, folder_id, subject, sender, recipients, date, body, snippet,
                 attachment_names, has_attachments, conversation_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9, 'conversation')",
            params![
                id,
                folder_id,
                subject,
                sender,
                recipients,
                date,
                body,
                attachment_names,
                i64::from(has_attachments),
            ],
        )
        .expect("message should insert");
        conn.execute(
            "INSERT INTO messages_fts (
                 rowid, subject, sender, recipients, body, attachment_names
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, subject, sender, recipients, body, attachment_names],
        )
        .expect("FTS row should insert");
    }

    fn query_ids(
        conn: &Connection,
        criteria: &MessageSearchCriteria,
        folder_id: Option<i64>,
        include_subfolders: bool,
    ) -> Vec<i64> {
        let source = build_message_query_source(conn, folder_id, include_subfolders, criteria)
            .expect("query should compile");
        let sql = format!(
            "SELECT m.id{}{} ORDER BY m.id",
            source.from_sql, source.where_sql
        );
        let mut statement = conn.prepare(&sql).expect("query should prepare");
        statement
            .query_map(params_from_iter(source.params.iter()), |row| row.get(0))
            .expect("query should execute")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("query rows should collect")
    }

    fn query_match_contexts(
        conn: &Connection,
        criteria: &MessageSearchCriteria,
    ) -> Vec<(i64, Option<SearchMatchContext>)> {
        let source = build_message_query_source(conn, None, false, criteria)
            .expect("context query should compile");
        let sql = format!(
            "SELECT m.id{}{}{} ORDER BY m.id",
            search_match_select_sql(source.has_text_match),
            source.from_sql,
            source.where_sql
        );
        let mut statement = conn.prepare(&sql).expect("context query should prepare");
        statement
            .query_map(params_from_iter(source.params.iter()), |row| {
                Ok((row.get(0)?, search_match_context_from_row(row, 1)?))
            })
            .expect("context query should execute")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("context rows should collect")
    }

    fn query_relevance_page(
        conn: &Connection,
        criteria: &MessageSearchCriteria,
        limit: i64,
        offset: i64,
    ) -> Vec<(i64, Option<SearchMatchContext>)> {
        let source = build_message_query_source(conn, None, false, criteria)
            .expect("relevance query should compile");
        let sort = message_sort_clause(Some("relevance"), source.has_text_match)
            .expect("text search should support relevance");
        let sql = format!(
            "SELECT m.id{}{}{} ORDER BY {sort} LIMIT ? OFFSET ?",
            search_match_select_sql(source.has_text_match),
            source.from_sql,
            source.where_sql
        );
        let mut query_params = source.params;
        query_params.push(Value::Integer(limit));
        query_params.push(Value::Integer(offset));
        let mut statement = conn.prepare(&sql).expect("relevance query should prepare");
        statement
            .query_map(params_from_iter(query_params.iter()), |row| {
                Ok((row.get(0)?, search_match_context_from_row(row, 1)?))
            })
            .expect("relevance query should execute")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("relevance rows should collect")
    }

    fn one_match_context(
        conn: &Connection,
        query: Option<&str>,
        filters: Option<SearchFilters>,
    ) -> SearchMatchContext {
        let criteria = MessageSearchCriteria::from_inputs(query.map(str::to_string), filters)
            .expect("search criteria should be valid");
        let contexts = query_match_contexts(conn, &criteria);
        assert_eq!(contexts.len(), 1, "search should return one fixture row");
        contexts
            .into_iter()
            .next()
            .and_then(|(_, context)| context)
            .expect("text search should return match context")
    }

    fn highlighted_text(context: &SearchMatchContext) -> Vec<&str> {
        context
            .highlight_ranges
            .iter()
            .map(|range| utf16_slice(&context.snippet_text, range.start, range.end))
            .collect()
    }

    fn utf16_slice(value: &str, start: usize, end: usize) -> &str {
        let mut byte_start = None;
        let mut byte_end = None;
        let mut utf16_offset = 0usize;
        for (byte_index, character) in value.char_indices() {
            if utf16_offset == start {
                byte_start = Some(byte_index);
            }
            if utf16_offset == end {
                byte_end = Some(byte_index);
                break;
            }
            utf16_offset += character.len_utf16();
        }
        if utf16_offset == end && byte_end.is_none() {
            byte_end = Some(value.len());
        }
        &value[byte_start.expect("range start must be a UTF-16 boundary")
            ..byte_end.expect("range end must be a UTF-16 boundary")]
    }

    fn populated_connection() -> Connection {
        let conn = fixture_connection();
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (1, NULL, 'Inbox', 'Inbox')",
            [],
        )
        .unwrap();
        insert_message(
            &conn,
            1,
            1,
            "Project calendar update",
            "Alice Example <alice@example.com>",
            "Team <team@example.com>",
            "2026-03-15T12:00:00+00:00",
            "Outlook migration body",
            "Quarterly Report.pdf application/pdf",
            true,
        );
        insert_message(
            &conn,
            2,
            1,
            "Other note",
            "Bob Example <bob@example.com>",
            "Else <else@example.com>",
            "2026-04-02T12:00:00+00:00",
            "Plain body",
            "",
            false,
        );
        conn.execute(
            "INSERT INTO attachments (
                 id, message_id, filename, sanitized_filename, content_type
             ) VALUES (1, 1, 'Quarterly Report.pdf', 'Quarterly Report.pdf', 'application/pdf')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn blank_and_ordinary_queries_preserve_and_prefix_semantics() {
        assert_eq!(fts_query(""), None);
        assert_eq!(fts_query("alpha beta").as_deref(), Some("alpha* AND beta*"));
        assert_eq!(
            fts_query("\"alpha beta\"").as_deref(),
            Some("\"alpha beta\"")
        );
        assert_eq!(fts_query("!!! --").as_deref(), None);
    }

    #[test]
    fn quoted_single_word_retains_legacy_prefix_behavior() {
        // Search 2.0 can change this intentionally later; extraction must not do so silently.
        assert_eq!(fts_query("\"calendar\"").as_deref(), Some("calendar*"));
    }

    #[test]
    fn leading_minus_retains_legacy_non_exclusion_behavior() {
        // A leading minus is punctuation today, not an exclusion operator.
        assert_eq!(fts_query("-draft").as_deref(), Some("draft*"));
    }

    #[test]
    fn punctuation_apostrophes_quotes_and_raw_fts_syntax_are_sanitized() {
        assert_eq!(fts_query("O'Brien").as_deref(), Some("o* AND brien*"));
        assert_eq!(
            fts_query("NEAR(foo) OR baz^").as_deref(),
            Some("near* AND foo* AND or* AND baz*")
        );
        assert_eq!(
            fts_query("alpha \"beta gamma").as_deref(),
            Some("alpha* AND \"beta gamma\"")
        );
        assert_eq!(
            fts_query("\"alpha \"beta\" gamma").as_deref(),
            Some("alpha* AND beta* AND gamma*")
        );
    }

    #[test]
    fn unicode_scripts_and_emoji_are_handled_without_byte_counting() {
        assert_eq!(
            fts_query("café 東京 العربية 😀mail").as_deref(),
            Some("café* AND 東京* AND العربية* AND mail*")
        );
        let query = format!("{} {}", "é".repeat(64), "東京");
        MessageSearchCriteria::from_inputs(Some(query), None)
            .expect("64 Unicode scalar values should be accepted");
    }

    #[test]
    fn typed_fields_aliases_unknown_fields_and_phrases_compile_consistently() {
        let parsed = criteria(
            "from:adam to:kevin cc:copy bcc:hidden recipient:one recipients:two \
             subj:calendar subject:\"project update\" text:outlook body:notes \
             attach:pdf filename:png attachment:doc has:attachment",
        );
        let fts = build_combined_fts_query(&parsed).expect("typed query should compile");
        for expected in [
            "sender:adam*",
            "recipients:kevin*",
            "recipients:copy*",
            "recipients:hidden*",
            "subject:calendar*",
            "subject:\"project update\"",
            "body:outlook*",
            "attachment_names:pdf*",
        ] {
            assert!(fts.contains(expected), "missing {expected} in {fts}");
        }
        assert_eq!(parsed.has_attachments, Some(true));
        assert_eq!(
            fts_query("mystery:value").as_deref(),
            Some("mystery* AND value*")
        );
    }

    #[test]
    fn query_complexity_limits_are_unicode_scalar_based_and_stable() {
        let max_query = std::iter::once("a".repeat(16))
            .chain(std::iter::repeat_n("b".repeat(15), 31))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(max_query.chars().count(), MAX_QUERY_SCALARS);
        MessageSearchCriteria::from_inputs(Some(max_query), None)
            .expect("maximum query should be accepted");

        assert_eq!(
            MessageSearchCriteria::from_inputs(Some("x".repeat(MAX_QUERY_SCALARS + 1)), None)
                .unwrap_err(),
            SearchValidationError::QueryTooLong
        );
        assert_eq!(
            MessageSearchCriteria::from_inputs(Some(vec!["x"; 33].join(" ")), None).unwrap_err(),
            SearchValidationError::TooManyClauses
        );
        assert_eq!(
            MessageSearchCriteria::from_inputs(
                Some(format!("from:{}", "x".repeat(MAX_TOKEN_SCALARS + 1))),
                None,
            )
            .unwrap_err(),
            SearchValidationError::TokenTooLong
        );
        assert_eq!(
            MessageSearchCriteria::from_inputs(
                Some(format!("\"{}\"", "x".repeat(MAX_PHRASE_SCALARS + 1))),
                None,
            )
            .unwrap_err(),
            SearchValidationError::PhraseTooLong
        );

        let filters = SearchFilters {
            subject: Some("x".repeat(MAX_STRUCTURED_FILTER_SCALARS + 1)),
            ..SearchFilters::default()
        };
        assert_eq!(
            MessageSearchCriteria::from_inputs(None, Some(filters)).unwrap_err(),
            SearchValidationError::StructuredFilterTooLong { field: "subject" }
        );
    }

    #[test]
    fn structured_filters_cover_fields_attachment_states_blanks_and_combinations() {
        let conn = populated_connection();
        for filters in [
            SearchFilters {
                from: Some("alice".into()),
                ..SearchFilters::default()
            },
            SearchFilters {
                recipients: Some("team".into()),
                ..SearchFilters::default()
            },
            SearchFilters {
                subject: Some("calendar".into()),
                ..SearchFilters::default()
            },
            SearchFilters {
                body: Some("outlook".into()),
                ..SearchFilters::default()
            },
            SearchFilters {
                attachment: Some("application pdf".into()),
                ..SearchFilters::default()
            },
            SearchFilters {
                from: Some("alice".into()),
                recipients: Some("team".into()),
                subject: Some("project".into()),
                body: Some("migration".into()),
                attachment: Some("report".into()),
                has_attachments: Some("yes".into()),
                ..SearchFilters::default()
            },
        ] {
            let parsed = MessageSearchCriteria::from_inputs(None, Some(filters)).unwrap();
            assert_eq!(query_ids(&conn, &parsed, None, false), vec![1]);
        }

        let without_attachments = MessageSearchCriteria::from_inputs(
            None,
            Some(SearchFilters {
                has_attachments: Some("no".into()),
                ..SearchFilters::default()
            }),
        )
        .unwrap();
        assert_eq!(query_ids(&conn, &without_attachments, None, false), vec![2]);

        let blank = MessageSearchCriteria::from_inputs(
            None,
            Some(SearchFilters {
                from: Some("   ".into()),
                body: Some(String::new()),
                ..SearchFilters::default()
            }),
        )
        .unwrap();
        assert_eq!(query_ids(&conn, &blank, None, false), vec![1, 2]);
    }

    #[test]
    fn date_only_bounds_include_both_complete_utc_dates() {
        let conn = fixture_connection();
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (1, NULL, 'Inbox', 'Inbox')",
            [],
        )
        .unwrap();
        for (id, date) in [
            (1, "2026-02-28T23:59:59+00:00"),
            (2, "2026-03-01T00:00:00+00:00"),
            (3, "2026-03-31T23:59:59.999999999+00:00"),
            (4, "2026-04-01T00:00:00+00:00"),
        ] {
            insert_message(
                &conn, id, 1, "Date", "Sender", "To", date, "Body", "", false,
            );
        }

        let parsed = MessageSearchCriteria::from_inputs(
            None,
            Some(SearchFilters {
                date_from: Some("2026-03-01".into()),
                date_to: Some("2026-03-31".into()),
                ..SearchFilters::default()
            }),
        )
        .unwrap();
        assert_eq!(query_ids(&conn, &parsed, None, false), vec![2, 3]);
        assert_eq!(
            parsed.date_from.unwrap().sql_value,
            "2026-03-01T00:00:00+00:00"
        );
        let upper = parsed.date_to.unwrap();
        assert_eq!(upper.sql_value, "2026-04-01T00:00:00+00:00");
        assert!(!upper.inclusive);
    }

    #[test]
    fn dates_validate_malformed_reversed_leap_day_and_year_boundaries() {
        let invalid = MessageSearchCriteria::from_inputs(
            None,
            Some(SearchFilters {
                date_from: Some("2026-02-30".into()),
                ..SearchFilters::default()
            }),
        )
        .unwrap_err();
        assert_eq!(
            invalid,
            SearchValidationError::InvalidDate { field: "date_from" }
        );

        let reversed = MessageSearchCriteria::from_inputs(
            None,
            Some(SearchFilters {
                date_from: Some("2026-04-01".into()),
                date_to: Some("2026-03-31".into()),
                ..SearchFilters::default()
            }),
        )
        .unwrap_err();
        assert_eq!(reversed, SearchValidationError::ReversedDateRange);

        let leap = MessageSearchCriteria::from_inputs(
            None,
            Some(SearchFilters {
                date_from: Some("2024-02-29".into()),
                date_to: Some("2024-02-29".into()),
                ..SearchFilters::default()
            }),
        )
        .unwrap();
        assert_eq!(leap.date_to.unwrap().sql_value, "2024-03-01T00:00:00+00:00");

        let year = MessageSearchCriteria::from_inputs(
            Some("after:2026-12-31 before:2026-12-31".into()),
            None,
        )
        .unwrap();
        assert_eq!(year.date_to.unwrap().sql_value, "2027-01-01T00:00:00+00:00");
    }

    #[test]
    fn folder_scope_escapes_like_metacharacters_and_preserves_exact_scope() {
        assert_eq!(escape_like(r"Root%_\Box"), r"Root\%\_\\Box");
        let conn = fixture_connection();
        for (id, parent, path) in [
            (1, None, r"Root%_\Box"),
            (2, Some(1), r"Root%_\Box/Child"),
            (3, None, r"RootXX\Box"),
            (4, Some(3), r"RootXX\Box/Child"),
        ] {
            conn.execute(
                "INSERT INTO folders (id, parent_id, path, name) VALUES (?1, ?2, ?3, ?3)",
                params![id, parent, path],
            )
            .unwrap();
            insert_message(
                &conn,
                id,
                id,
                "Folder",
                "Sender",
                "To",
                "2026-01-01T00:00:00+00:00",
                "Body",
                "",
                false,
            );
        }
        let parsed = criteria("");
        assert_eq!(query_ids(&conn, &parsed, Some(1), false), vec![1]);
        assert_eq!(query_ids(&conn, &parsed, Some(1), true), vec![1, 2]);
    }

    #[test]
    fn sort_allowlist_preserves_existing_order_and_fallback() {
        assert_eq!(
            message_sort_clause(None, false).unwrap(),
            "m.date DESC, m.id DESC"
        );
        assert_eq!(
            message_sort_clause(Some("newest"), false).unwrap(),
            "m.date DESC, m.id DESC"
        );
        assert_eq!(
            message_sort_clause(Some("oldest"), false).unwrap(),
            "m.date ASC, m.id ASC"
        );
        assert!(message_sort_clause(Some("sender_az"), false)
            .unwrap()
            .starts_with("m.sender"));
        assert!(message_sort_clause(Some("subject_az"), false)
            .unwrap()
            .starts_with("m.subject"));
        assert_eq!(
            message_sort_clause(Some("DROP TABLE"), false).unwrap(),
            "m.date DESC, m.id DESC"
        );
    }

    #[test]
    fn relevance_validation_requires_text_one_workspace_and_messages_mode() {
        assert_eq!(
            message_sort_clause(Some("relevance"), false).unwrap_err(),
            SearchValidationError::RelevanceRequiresText
        );
        assert_eq!(
            message_sort_clause(Some("relevance"), true).unwrap(),
            RELEVANCE_SORT_SQL
        );
        assert!(validate_message_sort_workspace_count(Some("relevance"), 1).is_ok());
        for workspace_count in [0, 2] {
            assert_eq!(
                validate_message_sort_workspace_count(Some("relevance"), workspace_count)
                    .unwrap_err(),
                SearchValidationError::RelevanceRequiresSingleWorkspace
            );
        }
        assert_eq!(
            validate_conversation_sort("relevance").unwrap_err(),
            SearchValidationError::RelevanceUnsupportedForConversations
        );
        assert!(validate_conversation_sort("newest").is_ok());
        assert!(validate_message_sort_workspace_count(Some("newest"), 2).is_ok());
    }

    #[test]
    fn relevance_rejects_non_text_filters_and_punctuation_only_queries() {
        let conn = fixture_connection();
        for criteria in [
            MessageSearchCriteria::from_inputs(
                None,
                Some(SearchFilters {
                    date_from: Some("2026-01-01".into()),
                    ..SearchFilters::default()
                }),
            )
            .unwrap(),
            MessageSearchCriteria::from_inputs(
                None,
                Some(SearchFilters {
                    has_attachments: Some("yes".into()),
                    ..SearchFilters::default()
                }),
            )
            .unwrap(),
            criteria("--- "),
        ] {
            let source = build_message_query_source(&conn, None, false, &criteria).unwrap();
            assert!(!source.has_text_match);
            assert_eq!(
                message_sort_clause(Some("relevance"), source.has_text_match).unwrap_err(),
                SearchValidationError::RelevanceRequiresText
            );
        }
    }

    #[test]
    fn relevance_uses_documented_fts_field_weights_and_preserves_context() {
        let conn = fixture_connection();
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (1, NULL, 'Inbox', 'Inbox')",
            [],
        )
        .unwrap();
        for (id, subject, sender, recipients, body, attachment_names) in [
            (1, "rankterm", "sender", "recipient", "body", "file"),
            (2, "subject", "sender", "recipient", "body", "rankterm"),
            (3, "subject", "rankterm", "recipient", "body", "file"),
            (4, "subject", "sender", "rankterm", "body", "file"),
            (5, "subject", "sender", "recipient", "rankterm", "file"),
        ] {
            insert_message(
                &conn,
                id,
                1,
                subject,
                sender,
                recipients,
                "2026-01-01T00:00:00+00:00",
                body,
                attachment_names,
                id == 2,
            );
        }

        let ranked = query_relevance_page(&conn, &criteria("rankterm"), 20, 0);
        assert_eq!(
            ranked.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        assert_eq!(
            ranked
                .iter()
                .map(|(_, context)| context.as_ref().unwrap().matched_fields[0])
                .collect::<Vec<_>>(),
            vec![
                SearchMatchedField::Subject,
                SearchMatchedField::Attachment,
                SearchMatchedField::Sender,
                SearchMatchedField::Recipients,
                SearchMatchedField::Body,
            ]
        );
    }

    #[test]
    fn relevance_uses_date_then_id_as_deterministic_ties() {
        let conn = fixture_connection();
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (1, NULL, 'Inbox', 'Inbox')",
            [],
        )
        .unwrap();
        for (id, date) in [
            (1, "2026-01-01T00:00:00+00:00"),
            (2, "2026-02-01T00:00:00+00:00"),
            (3, "2026-02-01T00:00:00+00:00"),
        ] {
            insert_message(
                &conn,
                id,
                1,
                "tie term",
                "sender",
                "recipient",
                date,
                "body",
                "",
                false,
            );
        }
        let ranked = query_relevance_page(&conn, &criteria("term"), 20, 0);
        assert_eq!(
            ranked.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn relevance_supports_phrases_prefixes_structured_fields_and_unicode() {
        let conn = fixture_connection();
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (1, NULL, 'Inbox', 'Inbox')",
            [],
        )
        .unwrap();
        insert_message(
            &conn,
            1,
            1,
            "Café project planning 東京",
            "Sender",
            "Recipient",
            "2026-01-01T00:00:00+00:00",
            "structured bodyvalue العربية",
            "",
            false,
        );

        for criteria in [
            criteria("\"project planning\""),
            criteria("plan"),
            criteria("café"),
            criteria("東京"),
            criteria("العربية"),
            MessageSearchCriteria::from_inputs(
                None,
                Some(SearchFilters {
                    subject: Some("planning".into()),
                    ..SearchFilters::default()
                }),
            )
            .unwrap(),
            MessageSearchCriteria::from_inputs(
                None,
                Some(SearchFilters {
                    body: Some("bodyvalue".into()),
                    ..SearchFilters::default()
                }),
            )
            .unwrap(),
        ] {
            let ranked = query_relevance_page(&conn, &criteria, 20, 0);
            assert_eq!(ranked.len(), 1);
            assert!(ranked[0].1.is_some());
        }
    }

    #[test]
    fn relevance_pagination_is_stable_and_does_not_duplicate_rows() {
        let conn = fixture_connection();
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (1, NULL, 'Inbox', 'Inbox')",
            [],
        )
        .unwrap();
        for id in 1..=6 {
            insert_message(
                &conn,
                id,
                1,
                "stable match",
                "sender",
                "recipient",
                "2026-01-01T00:00:00+00:00",
                "body",
                "",
                false,
            );
        }
        let criteria = criteria("match");
        let first = query_relevance_page(&conn, &criteria, 3, 0)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        let second = query_relevance_page(&conn, &criteria, 3, 3)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        assert_eq!(first, vec![6, 5, 4]);
        assert_eq!(second, vec![3, 2, 1]);
        assert!(first.iter().all(|id| !second.contains(id)));
    }

    #[test]
    fn sql_injection_like_input_remains_bound_fts_data() {
        let conn = populated_connection();
        let parsed = criteria("'); DROP TABLE messages; --");
        let source = build_message_query_source(&conn, None, false, &parsed).unwrap();
        assert!(!source.from_sql.contains("DROP"));
        assert!(!source.where_sql.contains("DROP"));
        assert!(source.params.iter().any(|value| {
            matches!(value, Value::Text(text) if text.contains("drop*") && text.contains("table*"))
        }));
        assert!(query_ids(&conn, &parsed, None, false).is_empty());
        assert!(query_relevance_page(&conn, &parsed, 20, 0).is_empty());
        let message_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(message_count, 2);
    }

    #[test]
    fn fts_match_context_identifies_each_searchable_field() {
        let conn = fixture_connection();
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (1, NULL, 'Inbox', 'Inbox')",
            [],
        )
        .unwrap();
        insert_message(
            &conn,
            1,
            1,
            "Agenda subjecttoken shared",
            "Sender sendername shared",
            "Recipient recipientname shared",
            "2026-01-01T00:00:00+00:00",
            "Body bodytoken shared",
            "Attachment attachmenttoken.pdf shared",
            true,
        );

        for (query, expected_field, expected_text) in [
            (
                "subject:subjecttoken",
                SearchMatchedField::Subject,
                "subjecttoken",
            ),
            ("from:sendername", SearchMatchedField::Sender, "sendername"),
            (
                "recipient:recipientname",
                SearchMatchedField::Recipients,
                "recipientname",
            ),
            ("body:bodytoken", SearchMatchedField::Body, "bodytoken"),
            (
                "attachment:attachmenttoken",
                SearchMatchedField::Attachment,
                "attachmenttoken",
            ),
        ] {
            let context = one_match_context(&conn, Some(query), None);
            assert_eq!(context.matched_fields, vec![expected_field]);
            assert!(highlighted_text(&context)
                .iter()
                .any(|value| value.eq_ignore_ascii_case(expected_text)));
        }

        let filters = SearchFilters {
            from: Some("sendername".into()),
            recipients: Some("recipientname".into()),
            subject: Some("subjecttoken".into()),
            body: Some("bodytoken".into()),
            attachment: Some("attachmenttoken".into()),
            ..SearchFilters::default()
        };
        let context = one_match_context(&conn, None, Some(filters));
        assert_eq!(
            context.matched_fields,
            vec![
                SearchMatchedField::Subject,
                SearchMatchedField::Sender,
                SearchMatchedField::Recipients,
                SearchMatchedField::Body,
                SearchMatchedField::Attachment,
            ]
        );

        let context = one_match_context(&conn, Some("shared"), None);
        assert_eq!(
            context.matched_fields,
            vec![
                SearchMatchedField::Subject,
                SearchMatchedField::Sender,
                SearchMatchedField::Recipients,
                SearchMatchedField::Body,
                SearchMatchedField::Attachment,
            ]
        );
    }

    #[test]
    fn fts_context_tracks_phrases_prefixes_and_unicode_text() {
        let conn = fixture_connection();
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (1, NULL, 'Inbox', 'Inbox')",
            [],
        )
        .unwrap();
        insert_message(
            &conn,
            1,
            1,
            "Unicode",
            "Sender",
            "Recipient",
            "2026-01-01T00:00:00+00:00",
            "Café project 東京 العربية 😀 calendar planning phrase",
            "",
            false,
        );

        for (query, expected) in [
            ("\"calendar planning\"", "calendar planning"),
            ("plan", "planning"),
            ("café", "Café"),
            ("東京", "東京"),
            ("العربية", "العربية"),
            ("calendar", "calendar"),
        ] {
            let context = one_match_context(&conn, Some(query), None);
            assert_eq!(context.matched_fields, vec![SearchMatchedField::Body]);
            assert!(highlighted_text(&context)
                .iter()
                .any(|value| value.eq_ignore_ascii_case(expected)));
        }

        let emoji_context = one_match_context(&conn, Some("calendar"), None);
        for range in &emoji_context.highlight_ranges {
            assert!(range.end <= emoji_context.snippet_text.encode_utf16().count());
        }
    }

    #[test]
    fn long_body_context_is_bounded_centered_and_whitespace_normalized() {
        let conn = fixture_connection();
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (1, NULL, 'Inbox', 'Inbox')",
            [],
        )
        .unwrap();
        let body = format!(
            "{}\n\n\t targetneedle \r\n {}",
            "before ".repeat(100),
            "after ".repeat(100)
        );
        insert_message(
            &conn,
            1,
            1,
            "Long body",
            "Sender",
            "Recipient",
            "2026-01-01T00:00:00+00:00",
            &body,
            "",
            false,
        );

        let context = one_match_context(&conn, Some("targetneedle"), None);
        assert!(context.snippet_text.chars().count() <= MAX_SEARCH_SNIPPET_SCALARS);
        assert!(context.snippet_text.starts_with('…'));
        assert!(context.snippet_text.ends_with('…'));
        assert!(!context.snippet_text.contains("  "));
        assert!(!context.snippet_text.contains('\n'));
        assert_eq!(highlighted_text(&context), vec!["targetneedle"]);
    }

    #[test]
    fn non_text_filters_keep_import_time_snippet_fallback() {
        let conn = populated_connection();
        for filters in [
            SearchFilters {
                date_from: Some("2026-03-01".into()),
                date_to: Some("2026-03-31".into()),
                ..SearchFilters::default()
            },
            SearchFilters {
                has_attachments: Some("yes".into()),
                ..SearchFilters::default()
            },
        ] {
            let criteria = MessageSearchCriteria::from_inputs(None, Some(filters)).unwrap();
            let contexts = query_match_contexts(&conn, &criteria);
            assert_eq!(contexts.len(), 1);
            assert!(contexts[0].1.is_none());
        }
    }

    #[test]
    fn malformed_markers_fall_back_without_leaking_marker_tokens() {
        let malformed = format!("before {SEARCH_HIGHLIGHT_START}unterminated");
        assert!(parse_marked_snippet(&malformed).is_none());
        let unexpected_end = format!("before {SEARCH_HIGHLIGHT_END} after");
        assert!(parse_marked_snippet(&unexpected_end).is_none());

        let fields = [(
            SearchMatchedField::Body,
            Some(format!(
                "safe {SEARCH_HIGHLIGHT_START}match{SEARCH_HIGHLIGHT_END} text"
            )),
        )];
        let context = build_search_match_context(Some(&malformed), &fields)
            .expect("a valid field snippet should provide a safe fallback");
        assert_eq!(context.snippet_text, "safe match text");
        assert!(!context.snippet_text.contains("PSTQV-HIGHLIGHT"));
    }

    #[test]
    fn marker_parser_uses_utf16_ranges_caps_ranges_and_returns_plain_text() {
        let marked = format!(
            "plain <tag> 😀 {start}東京{end} العربية {start}café{end}",
            start = SEARCH_HIGHLIGHT_START,
            end = SEARCH_HIGHLIGHT_END,
        );
        let parsed = parse_marked_snippet(&marked).expect("markers should parse");
        assert_eq!(
            highlighted_text(&SearchMatchContext {
                snippet_text: parsed.text.clone(),
                highlight_ranges: parsed.highlight_ranges.clone(),
                matched_fields: vec![SearchMatchedField::Body],
            }),
            vec!["東京", "café"]
        );
        assert!(parsed.text.contains("<tag>"));
        assert!(!parsed.text.contains("<mark>"));

        let many_ranges = (0..20)
            .map(|index| {
                format!(
                    "{start}term{index}{end}",
                    start = SEARCH_HIGHLIGHT_START,
                    end = SEARCH_HIGHLIGHT_END,
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        let parsed = parse_marked_snippet(&many_ranges).expect("many markers should parse");
        assert_eq!(parsed.highlight_ranges.len(), MAX_SEARCH_HIGHLIGHT_RANGES);
        assert!(parsed.highlight_ranges.iter().all(
            |range| range.start < range.end && range.end <= parsed.text.encode_utf16().count()
        ));
    }
}
