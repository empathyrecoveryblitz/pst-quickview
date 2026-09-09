use std::{
    fs::{File, Metadata, OpenOptions},
    io::{self, Read},
    os::unix::{fs::MetadataExt, fs::OpenOptionsExt},
    path::Path,
};

pub(crate) const MAX_MESSAGE_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_MESSAGE_TEXT_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_MIME_DEPTH: usize = 64;
pub(crate) const MAX_MIME_PARTS: usize = 20_000;
pub(crate) const MAX_MIME_HEADER_FIELDS: usize = 50_000;
pub(crate) const MAX_MIME_BOUNDARY_LINES: usize = 40_000;
pub(crate) const MAX_MESSAGE_ATTACHMENTS: usize = 10_000;
pub(crate) const MAX_MESSAGE_RECIPIENTS: usize = 20_000;
pub(crate) const MAX_TRANSPORT_HEADERS_BYTES: usize = 16 * 1024 * 1024;

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(crate) fn open_regular_message_file(path: &Path) -> io::Result<File> {
    let expected = std::fs::symlink_metadata(path)?;
    if expected.file_type().is_symlink() || !expected.is_file() || expected.nlink() != 1 {
        return Err(invalid_data(
            "message source must be one regular, nonlinked file",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file()
        || opened.dev() != expected.dev()
        || opened.ino() != expected.ino()
        || opened.nlink() != 1
        || opened.len() != expected.len()
    {
        return Err(invalid_data("message source changed while it was opened"));
    }
    Ok(file)
}

pub(crate) fn open_bounded_message_file(path: &Path) -> io::Result<File> {
    let file = open_regular_message_file(path)?;
    if file.metadata()?.len() > MAX_MESSAGE_SOURCE_BYTES {
        return Err(invalid_data("message source exceeds the safety limit"));
    }
    Ok(file)
}

fn validate_message_file_unchanged(file: &File, before: &Metadata) -> io::Result<()> {
    let after = file.metadata()?;
    if !after.is_file()
        || after.dev() != before.dev()
        || after.ino() != before.ino()
        || after.nlink() != 1
        || after.len() != before.len()
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
        || after.ctime() != before.ctime()
        || after.ctime_nsec() != before.ctime_nsec()
    {
        return Err(invalid_data("message source changed while it was read"));
    }
    Ok(())
}

/// Read only the top-level headers, including their terminating blank line.
///
/// Large message bodies stay on disk. Both the header allocation and each read
/// are bounded, including when a malformed source has no newline at all.
pub(crate) fn read_bounded_message_headers(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = open_regular_message_file(path)?;
    let before = file.metadata()?;
    let mut headers = Vec::with_capacity(8192);
    let mut buffer = [0u8; 8192];
    let mut line_start = 0usize;
    while headers.len() < MAX_TRANSPORT_HEADERS_BYTES {
        let remaining = MAX_TRANSPORT_HEADERS_BYTES - headers.len();
        let read_limit = buffer.len().min(remaining);
        let count = match file.read(&mut buffer[..read_limit]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Err(invalid_data("message headers are not terminated"));
        }
        for byte in &buffer[..count] {
            headers.push(*byte);
            if *byte != b'\n' {
                continue;
            }
            let line_end = headers.len() - 1;
            let blank_line = line_end == line_start
                || (line_end == line_start + 1 && headers[line_start] == b'\r');
            if blank_line {
                validate_message_file_unchanged(&file, &before)?;
                return Ok(headers);
            }
            line_start = headers.len();
        }
    }
    Err(invalid_data("message headers exceed the safety limit"))
}

pub(crate) fn read_bounded_message_source(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = open_bounded_message_file(path)?;
    let before = file.metadata()?;
    let mut bytes = Vec::with_capacity((before.len() as usize).min(1024 * 1024));
    file.by_ref()
        .take(MAX_MESSAGE_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MESSAGE_SOURCE_BYTES {
        return Err(invalid_data("message source exceeds the safety limit"));
    }
    validate_message_file_unchanged(&file, &before)?;
    Ok(bytes)
}

pub(crate) fn validate_mime_preflight(bytes: &[u8]) -> io::Result<()> {
    if bytes.len() as u64 > MAX_MESSAGE_SOURCE_BYTES {
        return Err(invalid_data("message source exceeds the safety limit"));
    }

    let mut header_fields = 0usize;
    let mut multipart_declarations = 0usize;
    let mut boundary_lines = 0usize;
    for raw_line in bytes.split(|byte| *byte == b'\n') {
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        if line.starts_with(b"--") {
            boundary_lines += 1;
            if boundary_lines > MAX_MIME_BOUNDARY_LINES {
                return Err(invalid_data("message contains too many MIME boundaries"));
            }
        }

        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            continue;
        };
        if colon == 0 || colon > 128 {
            continue;
        }
        let name = trim_ascii(&line[..colon]);
        if name.is_empty()
            || !name
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
        {
            continue;
        }
        header_fields += 1;
        if header_fields > MAX_MIME_HEADER_FIELDS {
            return Err(invalid_data("message contains too many header fields"));
        }
        if name.eq_ignore_ascii_case(b"content-type") {
            let value = trim_ascii(&line[colon + 1..]);
            if value
                .get(..10)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"multipart/"))
            {
                multipart_declarations += 1;
                if multipart_declarations > MAX_MIME_DEPTH {
                    return Err(invalid_data(
                        "message MIME nesting exceeds the safety limit",
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_parsed_mail(parsed: &mailparse::ParsedMail<'_>) -> io::Result<()> {
    let mut stack = vec![(parsed, 1usize)];
    let mut part_count = 0usize;
    let mut header_count = 0usize;
    let mut attachment_count = 0usize;
    while let Some((part, depth)) = stack.pop() {
        if depth > MAX_MIME_DEPTH {
            return Err(invalid_data(
                "message MIME nesting exceeds the safety limit",
            ));
        }
        part_count += 1;
        if part_count > MAX_MIME_PARTS {
            return Err(invalid_data("message contains too many MIME parts"));
        }
        header_count = header_count.saturating_add(part.headers.len());
        if header_count > MAX_MIME_HEADER_FIELDS {
            return Err(invalid_data("message contains too many header fields"));
        }
        let disposition = part.get_content_disposition();
        if matches!(
            disposition.disposition,
            mailparse::DispositionType::Attachment
        ) || disposition.params.contains_key("filename")
            || part.ctype.params.contains_key("name")
        {
            attachment_count += 1;
            if attachment_count > MAX_MESSAGE_ATTACHMENTS {
                return Err(invalid_data("message contains too many attachments"));
            }
        }
        for child in part.subparts.iter().rev() {
            stack.push((child, depth + 1));
        }
    }
    Ok(())
}

pub(crate) fn validate_outlook_message(outlook: &msg_parser::Outlook) -> io::Result<()> {
    if outlook.attachments.len() > MAX_MESSAGE_ATTACHMENTS {
        return Err(invalid_data("message contains too many attachments"));
    }
    if outlook.to.len() + outlook.cc.len() + outlook.bcc.len() > MAX_MESSAGE_RECIPIENTS {
        return Err(invalid_data("message contains too many recipients"));
    }
    for text in [&outlook.body, &outlook.html, &outlook.rtf_compressed] {
        if text.len() > MAX_MESSAGE_TEXT_BYTES {
            return Err(invalid_data("message body exceeds the safety limit"));
        }
    }
    if outlook.headers.raw.len() > MAX_TRANSPORT_HEADERS_BYTES {
        return Err(invalid_data("message headers exceed the safety limit"));
    }
    let mut payload_bytes = 0usize;
    for attachment in &outlook.attachments {
        payload_bytes = payload_bytes
            .checked_add(attachment.payload_bytes.len())
            .ok_or_else(|| invalid_data("attachment data exceeds the safety limit"))?;
        if payload_bytes as u64 > MAX_MESSAGE_SOURCE_BYTES {
            return Err(invalid_data("attachment data exceeds the safety limit"));
        }
    }
    Ok(())
}

pub(crate) fn validate_rendered_message_text(body: &str, html: &str) -> io::Result<()> {
    if body.len() > MAX_MESSAGE_TEXT_BYTES || html.len() > MAX_MESSAGE_TEXT_BYTES {
        return Err(invalid_data("message body exceeds the safety limit"));
    }
    Ok(())
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Cursor, Seek, SeekFrom, Write},
    };

    fn temporary_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pst-quickview-message-security-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let path = root.join("message.eml");
        let mut file = File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn bounded_source_open_rejects_links_and_detects_regular_files() {
        let contents = b"Subject: synthetic\r\n\r\nbody";
        let path = temporary_file("links", contents);
        assert_eq!(read_bounded_message_source(&path).unwrap(), contents);

        let symlink = path.with_file_name("symlink.eml");
        std::os::unix::fs::symlink(&path, &symlink).unwrap();
        assert!(open_regular_message_file(&symlink).is_err());
        assert!(open_bounded_message_file(&symlink).is_err());
        assert!(read_bounded_message_headers(&symlink).is_err());
        let hardlink = path.with_file_name("hardlink.eml");
        fs::hard_link(&path, &hardlink).unwrap();
        assert!(open_regular_message_file(&path).is_err());
        assert!(open_regular_message_file(&hardlink).is_err());
        assert!(open_bounded_message_file(&path).is_err());
        assert!(open_bounded_message_file(&hardlink).is_err());
        assert!(read_bounded_message_headers(&path).is_err());
        assert!(read_bounded_message_headers(&hardlink).is_err());
        assert!(open_regular_message_file(path.parent().unwrap()).is_err());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn oversized_source_supports_bounded_headers_without_changing_source() {
        let headers =
            b"Subject: synthetic large message\r\nContent-Type: application/octet-stream\r\n\r\n";
        let path = temporary_file("large-headers", headers);
        let mut source = OpenOptions::new().write(true).open(&path).unwrap();
        source.set_len(MAX_MESSAGE_SOURCE_BYTES + 17).unwrap();
        source.seek(SeekFrom::End(-4)).unwrap();
        source.write_all(b"tail").unwrap();
        drop(source);
        let before = fs::metadata(&path).unwrap();

        assert_eq!(
            open_regular_message_file(&path)
                .unwrap()
                .metadata()
                .unwrap()
                .len(),
            before.len()
        );
        assert!(open_bounded_message_file(&path).is_err());
        assert!(read_bounded_message_source(&path).is_err());
        assert_eq!(read_bounded_message_headers(&path).unwrap(), headers);

        let mut unchanged_source = File::open(&path).unwrap();
        validate_message_file_unchanged(&unchanged_source, &before).unwrap();
        let mut prefix = vec![0; headers.len()];
        unchanged_source.read_exact(&mut prefix).unwrap();
        assert_eq!(prefix, headers);
        unchanged_source.seek(SeekFrom::End(-4)).unwrap();
        let mut suffix = [0; 4];
        unchanged_source.read_exact(&mut suffix).unwrap();
        assert_eq!(&suffix, b"tail");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn header_reader_stops_at_blank_line_across_read_boundaries() {
        for (name, separator) in [
            ("lf-headers", b"\n\n".as_slice()),
            ("crlf-headers", b"\r\n\r\n".as_slice()),
        ] {
            let mut headers = b"X-Synthetic: ".to_vec();
            headers.resize(8191, b'x');
            headers.extend_from_slice(separator);
            let mut source = headers.clone();
            source.extend_from_slice(b"body must not be returned\n\n");
            let path = temporary_file(name, &source);
            assert_eq!(read_bounded_message_headers(&path).unwrap(), headers);
            assert_eq!(fs::read(&path).unwrap(), source);
            fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn header_reader_accepts_exact_budget_and_rejects_excessive_unterminated_headers() {
        let mut headers = b"X-Synthetic: ".to_vec();
        headers.resize(MAX_TRANSPORT_HEADERS_BYTES - 4, b'x');
        headers.extend_from_slice(b"\r\n\r\n");
        let path = temporary_file("exact-header-budget", &headers);
        assert_eq!(read_bounded_message_headers(&path).unwrap(), headers);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();

        let path = temporary_file("excessive-header-line", b"X-Synthetic: ");
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_TRANSPORT_HEADERS_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(
            read_bounded_message_headers(&path).unwrap_err().to_string(),
            "message headers exceed the safety limit"
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();

        for (index, bytes) in [
            b"".as_slice(),
            b"Subject: synthetic",
            b"Subject: synthetic\r\n",
        ]
        .iter()
        .enumerate()
        {
            let path = temporary_file(&format!("unterminated-headers-{index}"), bytes);
            assert_eq!(
                read_bounded_message_headers(&path).unwrap_err().to_string(),
                "message headers are not terminated"
            );
            fs::remove_dir_all(path.parent().unwrap()).unwrap();
        }
    }

    #[test]
    fn source_metadata_validation_rejects_length_and_link_changes() {
        let path = temporary_file("source-length-change", b"Subject: synthetic\n\n");
        let opened = open_regular_message_file(&path).unwrap();
        let before = opened.metadata().unwrap();
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(before.len() + 1)
            .unwrap();
        assert!(validate_message_file_unchanged(&opened, &before).is_err());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();

        let path = temporary_file("source-link-change", b"Subject: synthetic\n\n");
        let opened = open_regular_message_file(&path).unwrap();
        let before = opened.metadata().unwrap();
        fs::hard_link(&path, path.with_file_name("new-link.eml")).unwrap();
        assert!(validate_message_file_unchanged(&opened, &before).is_err());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn mime_preflight_rejects_excessive_headers_boundaries_and_nesting() {
        let headers = (0..=MAX_MIME_HEADER_FIELDS)
            .map(|index| format!("X-Synthetic-{index}: value\r\n"))
            .collect::<String>();
        assert!(validate_mime_preflight(headers.as_bytes()).is_err());

        let boundaries = (0..=MAX_MIME_BOUNDARY_LINES)
            .map(|_| "--synthetic\r\n")
            .collect::<String>();
        assert!(validate_mime_preflight(boundaries.as_bytes()).is_err());

        let nesting = (0..=MAX_MIME_DEPTH)
            .map(|_| "Content-Type: multipart/mixed; boundary=x\r\n")
            .collect::<String>();
        assert!(validate_mime_preflight(nesting.as_bytes()).is_err());
    }

    #[test]
    fn malformed_eml_mutations_never_panic() {
        let seed = b"From: synthetic@example.invalid\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nhello\r\n--x--\r\n";
        let mut cases = vec![Vec::new(), vec![0, 0xff, 0x80], seed.to_vec()];
        for end in (0..seed.len()).step_by(7) {
            cases.push(seed[..end].to_vec());
        }
        for index in (0..seed.len()).step_by(11) {
            let mut mutated = seed.to_vec();
            mutated[index] ^= 0xff;
            cases.push(mutated);
        }

        for bytes in cases {
            let outcome = std::panic::catch_unwind(|| {
                validate_mime_preflight(&bytes).and_then(|()| {
                    mailparse::parse_mail(&bytes)
                        .map_err(|_| invalid_data("synthetic parse failure"))
                        .and_then(|parsed| validate_parsed_mail(&parsed))
                })
            });
            assert!(outcome.is_ok());
        }
    }

    #[test]
    fn malformed_msg_and_cfb_mutations_never_panic() {
        let mut cases = vec![Vec::new(), vec![0, 0xff, 0x80], vec![0u8; 512]];
        let mut deterministic = 0x5a_u8;
        for length in [1usize, 7, 63, 511, 4096, 16_384] {
            let mut bytes = Vec::with_capacity(length);
            for index in 0..length {
                deterministic = deterministic
                    .wrapping_mul(17)
                    .wrapping_add(index as u8)
                    .wrapping_add(31);
                bytes.push(deterministic);
            }
            cases.push(bytes);
        }

        for bytes in cases {
            let msg = std::panic::catch_unwind(|| {
                let _ = msg_parser::Outlook::from_reader(Cursor::new(&bytes));
            });
            assert!(msg.is_ok());
            let cfb = std::panic::catch_unwind(|| {
                let _ = cfb::CompoundFile::open(Cursor::new(&bytes));
            });
            assert!(cfb.is_ok());
        }
    }
}
