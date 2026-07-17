use std::collections::{HashMap, HashSet};

pub const SUBJECT_FALLBACK_WINDOW_SECONDS: i64 = 14 * 24 * 60 * 60;

#[derive(Clone, Debug)]
pub struct ThreadInput {
    pub id: i64,
    pub eml_path: String,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub normalized_subject: String,
    pub sender_emails: HashSet<String>,
    pub recipient_emails: HashSet<String>,
    pub timestamp: Option<i64>,
    pub warning: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadAssignment {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub root_id: i64,
    pub conversation_seed: String,
    pub method: String,
    pub warning: String,
}

#[derive(Debug)]
struct DisjointSet {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl DisjointSet {
    fn new(length: usize) -> Self {
        Self {
            parent: (0..length).collect(),
            rank: vec![0; length],
        }
    }

    fn find(&mut self, index: usize) -> usize {
        if self.parent[index] != index {
            self.parent[index] = self.find(self.parent[index]);
        }
        self.parent[index]
    }

    fn union(&mut self, left: usize, right: usize) {
        let left_root = self.find(left);
        let right_root = self.find(right);
        if left_root == right_root {
            return;
        }
        match self.rank[left_root].cmp(&self.rank[right_root]) {
            std::cmp::Ordering::Less => self.parent[left_root] = right_root,
            std::cmp::Ordering::Greater => self.parent[right_root] = left_root,
            std::cmp::Ordering::Equal => {
                self.parent[right_root] = left_root;
                self.rank[left_root] += 1;
            }
        }
    }
}

pub fn normalize_message_id(value: &str) -> Option<String> {
    let value = value
        .trim()
        .trim_matches(|character| character == '<' || character == '>');
    let normalized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("")
        .to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

pub fn extract_message_ids(value: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut remainder = value;
    while let Some(start) = remainder.find('<') {
        let after_start = &remainder[start + 1..];
        let Some(end) = after_start.find('>') else {
            break;
        };
        if let Some(id) = normalize_message_id(&after_start[..end]) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        remainder = &after_start[end + 1..];
    }

    if ids.is_empty() {
        for token in value
            .split(|character: char| character.is_whitespace() || matches!(character, ',' | ';'))
        {
            if let Some(id) = normalize_message_id(token) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

pub fn normalize_thread_subject(subject: &str) -> String {
    let mut value = collapse_whitespace(subject);
    for _ in 0..16 {
        let Some(colon) = value.find(':') else {
            break;
        };
        let prefix = value[..colon].trim();
        if !is_reply_forward_prefix(prefix) {
            break;
        }
        value = collapse_whitespace(&value[colon + 1..]);
    }
    value
}

fn is_reply_forward_prefix(value: &str) -> bool {
    let compact = value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let prefixes = [
        "re", "fw", "fwd", "aw", "wg", "sv", "vs", "res", "tr", "antwort",
    ];
    prefixes.iter().any(|prefix| {
        let Some(suffix) = compact.strip_prefix(prefix) else {
            return false;
        };
        suffix.is_empty()
            || suffix.chars().all(|character| {
                character.is_ascii_digit() || matches!(character, '[' | ']' | '(' | ')')
            })
    })
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn extract_email_addresses(value: &str) -> HashSet<String> {
    value
        .split(|character: char| {
            character.is_whitespace()
                || matches!(character, '<' | '>' | ',' | ';' | '(' | ')' | '"' | '\'')
        })
        .map(|token| {
            token
                .trim_matches(|character: char| matches!(character, '.' | ':' | '[' | ']'))
                .to_ascii_lowercase()
        })
        .filter(|token| {
            let mut parts = token.split('@');
            parts.next().is_some_and(|part| !part.is_empty())
                && parts
                    .next()
                    .is_some_and(|part| part.contains('.') && !part.is_empty())
                && parts.next().is_none()
        })
        .collect()
}

pub fn assign_threads(inputs: &[ThreadInput]) -> Vec<ThreadAssignment> {
    if inputs.is_empty() {
        return Vec::new();
    }

    let mut dsu = DisjointSet::new(inputs.len());
    let id_to_index = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| (input.id, index))
        .collect::<HashMap<_, _>>();
    let mut message_id_indexes = HashMap::<String, Vec<usize>>::new();
    for (index, input) in inputs.iter().enumerate() {
        if let Some(message_id) = &input.message_id {
            message_id_indexes
                .entry(message_id.clone())
                .or_default()
                .push(index);
        }
    }

    let mut parent_indexes = vec![None; inputs.len()];
    let mut methods = vec!["standalone".to_string(); inputs.len()];
    let mut warnings = inputs
        .iter()
        .map(|input| input.warning.clone())
        .collect::<Vec<_>>();

    for indexes in message_id_indexes.values() {
        if indexes.len() < 2 {
            continue;
        }
        let first = indexes[0];
        for duplicate in indexes.iter().copied().skip(1) {
            dsu.union(first, duplicate);
            append_warning(
                &mut warnings[duplicate],
                "Duplicate Message-ID matched another indexed message.",
            );
        }
    }

    for (index, input) in inputs.iter().enumerate() {
        if let Some(in_reply_to) = &input.in_reply_to {
            if let Some(parent_index) = first_other_index(&message_id_indexes, in_reply_to, index) {
                dsu.union(index, parent_index);
                parent_indexes[index] = Some(parent_index);
                methods[index] = "header".to_string();
            } else {
                append_warning(
                    &mut warnings[index],
                    "In-Reply-To points to a message outside this workspace or an unavailable duplicate.",
                );
            }
        }

        let mut matched_references = Vec::new();
        for reference in &input.references {
            if let Some(reference_index) = first_other_index(&message_id_indexes, reference, index)
            {
                dsu.union(index, reference_index);
                matched_references.push(reference_index);
            }
        }
        if parent_indexes[index].is_none() {
            if let Some(parent_index) = matched_references.last().copied() {
                parent_indexes[index] = Some(parent_index);
                methods[index] = "references".to_string();
            } else if !input.references.is_empty() {
                append_warning(
                    &mut warnings[index],
                    "References point outside this workspace.",
                );
            }
        }
        if parent_indexes[index] == Some(index) {
            parent_indexes[index] = None;
            append_warning(
                &mut warnings[index],
                "Self-referential threading header was ignored.",
            );
        }
    }

    apply_subject_fallback(inputs, &mut dsu, &mut parent_indexes, &mut methods);

    let mut components = HashMap::<usize, Vec<usize>>::new();
    for index in 0..inputs.len() {
        let root = dsu.find(index);
        components.entry(root).or_default().push(index);
    }

    let mut assignments = Vec::with_capacity(inputs.len());
    for members in components.values() {
        let mut root_candidates = members
            .iter()
            .copied()
            .filter(|member| {
                parent_indexes[*member].is_none()
                    || parent_indexes[*member].is_some_and(|parent| !members.contains(&parent))
            })
            .collect::<Vec<_>>();
        let circular = root_candidates.is_empty();
        if circular {
            root_candidates.extend(members.iter().copied());
        }
        root_candidates
            .sort_by(|left, right| compare_thread_order(&inputs[*left], &inputs[*right]));
        let root_index = root_candidates[0];
        let seed = members
            .iter()
            .filter_map(|index| inputs[*index].message_id.as_ref())
            .min()
            .map(|message_id| format!("header:{message_id}"))
            .unwrap_or_else(|| format!("path:{}", inputs[root_index].eml_path));

        for index in members {
            if circular {
                append_warning(
                    &mut warnings[*index],
                    "Circular or malformed reply chain detected; deterministic root selected.",
                );
            }
            if methods[*index] == "subject_fallback" {
                append_warning(
                    &mut warnings[*index],
                    "Conversation assigned by a conservative subject, participant, and date heuristic.",
                );
            }
            assignments.push(ThreadAssignment {
                id: inputs[*index].id,
                parent_id: parent_indexes[*index].map(|parent| inputs[parent].id),
                root_id: inputs[root_index].id,
                conversation_seed: seed.clone(),
                method: methods[*index].clone(),
                warning: warnings[*index].clone(),
            });
        }
    }

    assignments.sort_by_key(|assignment| assignment.id);
    debug_assert_eq!(assignments.len(), id_to_index.len());
    assignments
}

fn apply_subject_fallback(
    inputs: &[ThreadInput],
    dsu: &mut DisjointSet,
    parent_indexes: &mut [Option<usize>],
    methods: &mut [String],
) {
    let mut groups = HashMap::<String, Vec<usize>>::new();
    for (index, input) in inputs.iter().enumerate() {
        if input.in_reply_to.is_some()
            || !input.references.is_empty()
            || input.normalized_subject.trim().is_empty()
            || input.timestamp.is_none()
            || input.sender_emails.is_empty()
        {
            continue;
        }
        groups
            .entry(input.normalized_subject.to_ascii_lowercase())
            .or_default()
            .push(index);
    }

    for indexes in groups.values_mut() {
        indexes.sort_by(|left, right| compare_thread_order(&inputs[*left], &inputs[*right]));
        for position in 0..indexes.len() {
            let current = indexes[position];
            if dsu.find(current) != current || component_size(dsu, inputs.len(), current) > 1 {
                continue;
            }
            for previous_position in (0..position).rev() {
                let previous = indexes[previous_position];
                let Some(current_time) = inputs[current].timestamp else {
                    break;
                };
                let Some(previous_time) = inputs[previous].timestamp else {
                    continue;
                };
                if current_time.saturating_sub(previous_time) > SUBJECT_FALLBACK_WINDOW_SECONDS {
                    break;
                }
                if conservative_participant_overlap(&inputs[current], &inputs[previous]) {
                    dsu.union(current, previous);
                    parent_indexes[current] = Some(previous);
                    methods[current] = "subject_fallback".to_string();
                    break;
                }
            }
        }
    }
}

fn component_size(dsu: &mut DisjointSet, length: usize, index: usize) -> usize {
    let root = dsu.find(index);
    (0..length)
        .filter(|candidate| dsu.find(*candidate) == root)
        .count()
}

fn conservative_participant_overlap(left: &ThreadInput, right: &ThreadInput) -> bool {
    let shared_senders = left
        .sender_emails
        .intersection(&right.sender_emails)
        .count();
    let mut left_all = left.sender_emails.clone();
    left_all.extend(left.recipient_emails.iter().cloned());
    let mut right_all = right.sender_emails.clone();
    right_all.extend(right.recipient_emails.iter().cloned());
    let shared_all = left_all.intersection(&right_all).count();
    let reciprocal = left
        .sender_emails
        .iter()
        .any(|sender| right.recipient_emails.contains(sender))
        && right
            .sender_emails
            .iter()
            .any(|sender| left.recipient_emails.contains(sender));
    reciprocal || (shared_senders > 0 && shared_all >= 2)
}

fn first_other_index(
    indexes: &HashMap<String, Vec<usize>>,
    message_id: &str,
    current: usize,
) -> Option<usize> {
    indexes
        .get(message_id)
        .and_then(|matches| matches.iter().copied().find(|index| *index != current))
}

fn compare_thread_order(left: &ThreadInput, right: &ThreadInput) -> std::cmp::Ordering {
    match (left.timestamp, right.timestamp) {
        (Some(left_time), Some(right_time)) => left_time
            .cmp(&right_time)
            .then_with(|| left.id.cmp(&right.id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left.id.cmp(&right.id),
    }
}

fn append_warning(target: &mut String, warning: &str) {
    if target.contains(warning) {
        return;
    }
    if !target.trim().is_empty() {
        target.push(' ');
    }
    target.push_str(warning);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        id: i64,
        subject: &str,
        sender: &str,
        recipients: &str,
        timestamp: i64,
    ) -> ThreadInput {
        ThreadInput {
            id,
            eml_path: format!("{id}.eml"),
            message_id: Some(format!("message-{id}@example.com")),
            in_reply_to: None,
            references: Vec::new(),
            normalized_subject: normalize_thread_subject(subject),
            sender_emails: extract_email_addresses(sender),
            recipient_emails: extract_email_addresses(recipients),
            timestamp: Some(timestamp),
            warning: String::new(),
        }
    }

    #[test]
    fn normalizes_nested_subject_prefixes_and_message_ids() {
        assert_eq!(
            normalize_thread_subject(" Re: FWD: AW: Project Plan "),
            "Project Plan"
        );
        assert_eq!(
            normalize_message_id(" <ABC@Example.COM> ").as_deref(),
            Some("abc@example.com")
        );
        assert_eq!(
            extract_message_ids("<root@example.com> <reply@EXAMPLE.com>"),
            vec!["root@example.com", "reply@example.com"]
        );
    }

    #[test]
    fn headers_take_precedence_and_find_the_root() {
        let root = input(1, "Topic", "a@example.com", "b@example.com", 10);
        let mut reply = input(2, "Re: Topic", "b@example.com", "a@example.com", 20);
        reply.in_reply_to = root.message_id.clone();
        let assignments = assign_threads(&[root, reply]);
        assert_eq!(assignments[0].root_id, 1);
        assert_eq!(assignments[1].parent_id, Some(1));
        assert_eq!(assignments[1].method, "header");
        assert_eq!(
            assignments[0].conversation_seed,
            assignments[1].conversation_seed
        );
    }

    #[test]
    fn references_connect_messages_without_in_reply_to() {
        let root = input(1, "Topic", "a@example.com", "b@example.com", 10);
        let mut reply = input(2, "Re: Topic", "b@example.com", "a@example.com", 20);
        reply.references = vec![root.message_id.clone().unwrap()];
        let assignments = assign_threads(&[root, reply]);
        assert_eq!(assignments[1].parent_id, Some(1));
        assert_eq!(assignments[1].method, "references");
    }

    #[test]
    fn subject_fallback_requires_participant_overlap() {
        let root = input(1, "Status", "a@example.com", "b@example.com", 10);
        let related = input(2, "Re: Status", "b@example.com", "a@example.com", 20);
        let unrelated = input(3, "Status", "c@example.com", "d@example.com", 30);
        let assignments = assign_threads(&[root, related, unrelated]);
        assert_eq!(
            assignments[0].conversation_seed,
            assignments[1].conversation_seed
        );
        assert_eq!(assignments[1].method, "subject_fallback");
        assert_ne!(
            assignments[0].conversation_seed,
            assignments[2].conversation_seed
        );
    }
}
