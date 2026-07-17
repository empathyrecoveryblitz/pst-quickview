use chrono::{DateTime, Utc};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek},
    path::Path,
};

const MAX_CALENDAR_PROPERTY_STREAM_BYTES: usize = 2 * 1024 * 1024;
const PS_PUBLIC_STRINGS: [u8; 16] = [
    0x29, 0x03, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];
const PSETID_COMMON: [u8; 16] = [
    0x08, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];
const PSETID_APPOINTMENT: [u8; 16] = [
    0x02, 0x20, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];
const PSETID_MEETING: [u8; 16] = [
    0x90, 0xDA, 0xD8, 0x6E, 0x0B, 0x45, 0x1B, 0x10, 0x98, 0xDA, 0x00, 0xAA, 0x00, 0x3F, 0x13, 0x05,
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalendarPropertyDiagnostic {
    pub name: String,
    pub property_id: String,
    pub property_type: String,
    pub value: String,
    pub source: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CalendarItemDetails {
    pub item_type: String,
    pub message_class: String,
    pub organizer: String,
    pub organizer_source: String,
    pub required_attendees: String,
    pub required_attendees_source: String,
    pub optional_attendees: String,
    pub optional_attendees_source: String,
    pub resources: String,
    pub start: String,
    pub end: String,
    pub start_raw: String,
    pub end_raw: String,
    pub time_zone: String,
    pub time_zone_source: String,
    pub time_zone_uncertain: bool,
    pub all_day: Option<bool>,
    pub location: String,
    pub recurrence_summary: String,
    pub recurrence_available: bool,
    pub recurrence_raw_summary: String,
    pub meeting_status: String,
    pub response_status: String,
    pub reminder: String,
    pub sensitivity: String,
    pub categories: Vec<String>,
    pub creation_time: String,
    pub modification_time: String,
    pub property_diagnostics: Vec<CalendarPropertyDiagnostic>,
    pub parse_warnings: Vec<String>,
    pub unsupported_properties: Vec<String>,
}

#[derive(Clone, Debug)]
struct NamedPropertyDefinition {
    property_id: u16,
    name: String,
}

#[derive(Clone, Debug)]
enum CalendarPropertyValue {
    Text(String),
    Integer(i64),
    Boolean(bool),
    Time(String),
    Binary(usize),
}

#[derive(Clone, Debug)]
struct CalendarProperty {
    name: String,
    property_id: Option<u16>,
    property_type: String,
    value: CalendarPropertyValue,
    source: String,
}

#[derive(Default)]
struct CalendarPropertyRead {
    properties: HashMap<String, CalendarProperty>,
    warnings: Vec<String>,
}

struct CalendarBuildInput {
    message_class: String,
    organizer: String,
    to: String,
    cc: String,
    sensitivity: u32,
    creation_time: String,
    modification_time: String,
    properties: HashMap<String, CalendarProperty>,
    parse_warnings: Vec<String>,
}

pub(crate) fn calendar_item_details(
    source_path: &Path,
    outlook: &msg_parser::Outlook,
) -> Option<CalendarItemDetails> {
    let item_type = calendar_item_type(&outlook.message_class)?;
    let mut property_read = read_calendar_properties(source_path);

    for (name, value) in &outlook.named_properties {
        if property_read.properties.contains_key(name) {
            continue;
        }
        property_read.properties.insert(
            name.clone(),
            CalendarProperty {
                name: name.clone(),
                property_id: known_property_id(name),
                property_type: "msg_parser value".to_string(),
                value: parser_property_value(name, value),
                source: "msg_parser named property".to_string(),
            },
        );
    }

    Some(build_calendar_details(
        item_type,
        CalendarBuildInput {
            message_class: outlook.message_class.trim().to_string(),
            organizer: format_person(&outlook.sender),
            to: format_people(&outlook.to),
            cc: format_people(&outlook.cc),
            sensitivity: outlook.sensitivity,
            creation_time: outlook.creation_time.clone(),
            modification_time: outlook.last_modification_time.clone(),
            properties: property_read.properties,
            parse_warnings: property_read.warnings,
        },
    ))
}

pub(crate) fn diagnostic_lines(calendar: &CalendarItemDetails) -> Vec<String> {
    let mut lines = vec![
        "Calendar item diagnostics".to_string(),
        format!("Detected item type: {}", calendar.item_type),
        format!("Message class: {}", calendar.message_class),
        format!("Start raw value: {}", value_or_none(&calendar.start_raw)),
        format!("End raw value: {}", value_or_none(&calendar.end_raw)),
        format!("Time-zone source: {}", calendar.time_zone_source),
        format!(
            "Time-zone interpretation uncertain: {}",
            calendar.time_zone_uncertain
        ),
        format!("All-day property: {}", option_bool_label(calendar.all_day)),
        format!(
            "Organizer source: {}",
            value_or_none(&calendar.organizer_source)
        ),
        format!(
            "Required attendees source: {}",
            value_or_none(&calendar.required_attendees_source)
        ),
        format!(
            "Optional attendees source: {}",
            value_or_none(&calendar.optional_attendees_source)
        ),
        format!("Recurrence available: {}", calendar.recurrence_available),
        format!(
            "Recurrence data: {}",
            value_or_none(&calendar.recurrence_raw_summary)
        ),
    ];

    if !calendar.property_diagnostics.is_empty() {
        lines.push("Calendar properties:".to_string());
        lines.extend(calendar.property_diagnostics.iter().map(|property| {
            format!(
                "- {} | id={} | type={} | value={} | source={}",
                property.name,
                property.property_id,
                property.property_type,
                property.value,
                property.source
            )
        }));
    }
    if !calendar.parse_warnings.is_empty() {
        lines.push("Calendar parse warnings:".to_string());
        lines.extend(
            calendar
                .parse_warnings
                .iter()
                .map(|warning| format!("- {warning}")),
        );
    }
    if !calendar.unsupported_properties.is_empty() {
        lines.push("Calendar limitations:".to_string());
        lines.extend(
            calendar
                .unsupported_properties
                .iter()
                .map(|warning| format!("- {warning}")),
        );
    }
    lines
}

fn calendar_item_type(message_class: &str) -> Option<&'static str> {
    let class = message_class.trim().to_ascii_lowercase();
    if class.starts_with("ipm.schedule.meeting.resp.pos") {
        Some("Meeting Accepted")
    } else if class.starts_with("ipm.schedule.meeting.resp.neg") {
        Some("Meeting Declined")
    } else if class.starts_with("ipm.schedule.meeting.resp.tent") {
        Some("Meeting Tentative")
    } else if class.starts_with("ipm.schedule.meeting.canceled")
        || class.starts_with("ipm.schedule.meeting.cancelled")
    {
        Some("Meeting Cancellation")
    } else if class.starts_with("ipm.schedule.meeting.request") {
        Some("Meeting Request")
    } else if class.starts_with("ipm.appointment") {
        Some("Appointment")
    } else {
        None
    }
}

fn build_calendar_details(item_type: &str, input: CalendarBuildInput) -> CalendarItemDetails {
    let start = first_property_text(
        &input.properties,
        &["AppointmentStartWhole", "CommonStart", "StartTime"],
    );
    let end = first_property_text(
        &input.properties,
        &["AppointmentEndWhole", "CommonEnd", "EndTime"],
    );
    let location = first_property_text(&input.properties, &["Location", "Where"]);
    let required_property = first_property_text(&input.properties, &["RequiredAttendees"]);
    let optional_property = first_property_text(&input.properties, &["OptionalAttendees"]);
    let resources = first_property_text(&input.properties, &["ResourceAttendees"]);
    let is_response = matches!(
        item_type,
        "Meeting Accepted" | "Meeting Declined" | "Meeting Tentative"
    );
    let allow_recipient_attendee_fallback = !is_response;
    let required_attendees = if required_property.is_empty() {
        if allow_recipient_attendee_fallback {
            input.to.clone()
        } else {
            String::new()
        }
    } else {
        required_property
    };
    let optional_attendees = if optional_property.is_empty() {
        if allow_recipient_attendee_fallback {
            input.cc.clone()
        } else {
            String::new()
        }
    } else {
        optional_property
    };
    let required_attendees_source = attendee_source(
        &input.properties,
        "RequiredAttendees",
        if allow_recipient_attendee_fallback {
            &input.to
        } else {
            ""
        },
        "MSG To recipient table fallback",
    );
    let optional_attendees_source = attendee_source(
        &input.properties,
        "OptionalAttendees",
        if allow_recipient_attendee_fallback {
            &input.cc
        } else {
            ""
        },
        "MSG Cc recipient table fallback",
    );
    let (organizer, organizer_source) = if is_response && !input.to.trim().is_empty() {
        (
            input.to.clone(),
            "MSG To recipient table for meeting response".to_string(),
        )
    } else {
        (input.organizer, "MSG sender property".to_string())
    };

    let all_day = property_bool(&input.properties, &["AppointmentSubType"]);
    let recurring_property = property_bool(&input.properties, &["Recurring", "IsRecurring"]);
    let recurrence_bytes = property_binary_len(&input.properties, "AppointmentRecur");
    let recurrence_available = recurring_property == Some(true) || recurrence_bytes.is_some();
    let recurrence_summary = if recurrence_available {
        "Recurring meeting".to_string()
    } else {
        String::new()
    };
    let recurrence_raw_summary = recurrence_bytes
        .map(|bytes| format!("Outlook recurrence pattern present ({bytes} bytes)"))
        .unwrap_or_else(|| {
            recurring_property
                .map(|value| format!("Recurring property: {value}"))
                .unwrap_or_default()
        });

    let time_zone_description = first_property_text(&input.properties, &["TimeZoneDescription"]);
    let binary_time_zone = property_binary_len(&input.properties, "TimeZoneStruct")
        .or_else(|| property_binary_len(&input.properties, "TimeZone"));
    let (time_zone, time_zone_source, time_zone_uncertain) = if !time_zone_description.is_empty() {
        (
            time_zone_description,
            "PidLidTimeZoneDescription".to_string(),
            false,
        )
    } else if let Some(bytes) = binary_time_zone {
        (
            "Outlook time-zone data present".to_string(),
            format!("Undecoded Outlook time-zone structure ({bytes} bytes)"),
            true,
        )
    } else {
        (
            "Not specified".to_string(),
            "No source time-zone property; appointment times are UTC values".to_string(),
            true,
        )
    };

    let response_status = property_integer(&input.properties, &["ResponseStatus"])
        .map(response_status_label)
        .unwrap_or_default();
    let appointment_state =
        property_integer(&input.properties, &["AppointmentStateFlags"]).unwrap_or(0);
    let busy_status = property_integer(&input.properties, &["BusyStatus"])
        .map(busy_status_label)
        .unwrap_or_default();
    let meeting_status = if item_type == "Meeting Cancellation" || appointment_state & 0x4 != 0 {
        "Canceled".to_string()
    } else if item_type != "Appointment" {
        item_type.to_string()
    } else if !busy_status.is_empty() {
        busy_status
    } else {
        item_type.to_string()
    };

    let reminder = reminder_summary(&input.properties);
    let categories = first_property_text(&input.properties, &["Keywords", "Categories"])
        .split([',', ';'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut parse_warnings = input.parse_warnings;
    validate_calendar_date("start", &start, &mut parse_warnings);
    validate_calendar_date("end", &end, &mut parse_warnings);

    let mut unsupported_properties = Vec::new();
    if recurrence_bytes.is_some() {
        unsupported_properties.push(
            "The Outlook recurrence pattern is preserved but is not expanded into individual occurrences."
                .to_string(),
        );
    }
    if binary_time_zone.is_some() && time_zone_uncertain {
        unsupported_properties.push(
            "The binary Outlook time-zone structure is present but is not decoded; displayed times use the stored UTC values."
                .to_string(),
        );
    }

    let mut property_diagnostics = input
        .properties
        .values()
        .filter(|property| is_calendar_property_name(&property.name))
        .map(property_diagnostic)
        .collect::<Vec<_>>();
    property_diagnostics.sort_by(|left, right| left.name.cmp(&right.name));

    CalendarItemDetails {
        item_type: item_type.to_string(),
        message_class: input.message_class,
        organizer,
        organizer_source,
        required_attendees,
        required_attendees_source,
        optional_attendees,
        optional_attendees_source,
        resources,
        start: start.clone(),
        end: end.clone(),
        start_raw: start,
        end_raw: end,
        time_zone,
        time_zone_source,
        time_zone_uncertain,
        all_day,
        location,
        recurrence_summary,
        recurrence_available,
        recurrence_raw_summary,
        meeting_status,
        response_status,
        reminder,
        sensitivity: sensitivity_label(input.sensitivity).to_string(),
        categories,
        creation_time: input.creation_time,
        modification_time: input.modification_time,
        property_diagnostics,
        parse_warnings,
        unsupported_properties,
    }
}

fn read_calendar_properties(source_path: &Path) -> CalendarPropertyRead {
    let mut result = CalendarPropertyRead::default();
    let mut compound = match cfb::open(source_path) {
        Ok(compound) => compound,
        Err(error) => {
            result.warnings.push(format!(
                "Calendar property storage could not be opened: {error}"
            ));
            return result;
        }
    };

    let guid_stream =
        read_optional_stream(&mut compound, "/__nameid_version1.0/__substg1.0_00020102");
    let entry_stream =
        read_optional_stream(&mut compound, "/__nameid_version1.0/__substg1.0_00030102");
    let string_stream =
        read_optional_stream(&mut compound, "/__nameid_version1.0/__substg1.0_00040102");
    let definitions = parse_named_property_definitions(&guid_stream, &entry_stream, &string_stream);

    let root_streams = compound
        .read_root_storage()
        .filter(|entry| entry.is_stream())
        .map(|entry| (entry.path().to_path_buf(), entry.name().to_string()))
        .collect::<Vec<_>>();
    for (path, name) in root_streams {
        let Some((property_id, property_type)) = parse_substitution_stream_name(&name) else {
            continue;
        };
        let Some(definition) = definitions.get(&property_id) else {
            continue;
        };
        let bytes = match read_bounded_stream(&mut compound, &path) {
            Ok(bytes) => bytes,
            Err(error) => {
                result.warnings.push(format!(
                    "Calendar property {} could not be read: {error}",
                    definition.name
                ));
                continue;
            }
        };
        if let Some(value) = decode_property_value(property_type, &bytes) {
            insert_property(
                &mut result.properties,
                definition,
                property_type,
                value,
                "MSG named-property stream",
            );
        }
    }

    if let Ok(bytes) = read_bounded_stream(&mut compound, Path::new("/__properties_version1.0")) {
        for chunk in bytes.get(32..).unwrap_or_default().chunks_exact(16) {
            let property_type = u16::from_le_bytes([chunk[0], chunk[1]]);
            let property_id = u16::from_le_bytes([chunk[2], chunk[3]]);
            let Some(definition) = definitions.get(&property_id) else {
                continue;
            };
            if let Some(value) = decode_property_value(property_type, &chunk[8..16]) {
                insert_property(
                    &mut result.properties,
                    definition,
                    property_type,
                    value,
                    "MSG root property table",
                );
            }
        }
    }

    result
}

fn read_optional_stream(compound: &mut cfb::CompoundFile<File>, path: &str) -> Vec<u8> {
    read_bounded_stream(compound, Path::new(path)).unwrap_or_default()
}

fn read_bounded_stream<F: Read + Seek>(
    compound: &mut cfb::CompoundFile<F>,
    path: &Path,
) -> std::io::Result<Vec<u8>> {
    let mut stream = compound.open_stream(path)?;
    let mut bytes = Vec::new();
    stream
        .by_ref()
        .take(MAX_CALENDAR_PROPERTY_STREAM_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CALENDAR_PROPERTY_STREAM_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "property stream exceeds the calendar diagnostics limit",
        ));
    }
    Ok(bytes)
}

fn parse_named_property_definitions(
    guid_stream: &[u8],
    entry_stream: &[u8],
    string_stream: &[u8],
) -> HashMap<u16, NamedPropertyDefinition> {
    let mut definitions = HashMap::new();
    for chunk in entry_stream.chunks_exact(8) {
        let name_id = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let packed = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        let property_id = 0x8000 + ((packed >> 16) as u16);
        let kind = packed & 1;
        let guid_index = ((packed >> 1) & 0x7fff) as usize;
        let Some(guid) = nameid_guid(guid_index, guid_stream) else {
            continue;
        };
        let name = if kind == 0 {
            calendar_property_name(&guid, name_id).map(str::to_string)
        } else {
            read_nameid_string(string_stream, name_id as usize)
        };
        if let Some(name) = name {
            definitions.insert(property_id, NamedPropertyDefinition { property_id, name });
        }
    }
    definitions
}

fn nameid_guid(index: usize, guid_stream: &[u8]) -> Option<[u8; 16]> {
    match index {
        1 => Some(PS_PUBLIC_STRINGS),
        3.. => {
            let offset = (index - 3) * 16;
            let bytes = guid_stream.get(offset..offset + 16)?;
            let mut guid = [0u8; 16];
            guid.copy_from_slice(bytes);
            Some(guid)
        }
        _ => None,
    }
}

fn read_nameid_string(bytes: &[u8], offset: usize) -> Option<String> {
    let length_bytes = bytes.get(offset..offset + 4)?;
    let length = u32::from_le_bytes(length_bytes.try_into().ok()?) as usize;
    if length > MAX_CALENDAR_PROPERTY_STREAM_BYTES {
        return None;
    }
    let value = bytes.get(offset + 4..offset + 4 + length)?;
    Some(decode_utf16le(value).trim_end_matches('\0').to_string()).filter(|value| !value.is_empty())
}

fn calendar_property_name(guid: &[u8; 16], dispid: u32) -> Option<&'static str> {
    match (guid, dispid) {
        (&PSETID_COMMON, 0x8501) => Some("ReminderDelta"),
        (&PSETID_COMMON, 0x8502) => Some("ReminderTime"),
        (&PSETID_COMMON, 0x8503) => Some("ReminderSet"),
        (&PSETID_COMMON, 0x8516) => Some("CommonStart"),
        (&PSETID_COMMON, 0x8517) => Some("CommonEnd"),
        (&PSETID_APPOINTMENT, 0x8205) => Some("BusyStatus"),
        (&PSETID_APPOINTMENT, 0x8208) => Some("Location"),
        (&PSETID_APPOINTMENT, 0x820D) => Some("AppointmentStartWhole"),
        (&PSETID_APPOINTMENT, 0x820E) => Some("AppointmentEndWhole"),
        (&PSETID_APPOINTMENT, 0x8213) => Some("AppointmentDuration"),
        (&PSETID_APPOINTMENT, 0x8215) => Some("AppointmentRecur"),
        (&PSETID_APPOINTMENT, 0x8216) => Some("AppointmentStateFlags"),
        (&PSETID_APPOINTMENT, 0x8217) => Some("ResponseStatus"),
        (&PSETID_APPOINTMENT, 0x8223) => Some("Recurring"),
        (&PSETID_APPOINTMENT, 0x8231) => Some("AppointmentSubType"),
        (&PSETID_APPOINTMENT, 0x8234) => Some("TimeZoneDescription"),
        (&PSETID_APPOINTMENT, 0x8235) => Some("TimeZoneStruct"),
        (&PSETID_APPOINTMENT, 0x8256) => Some("AllAttendeesString"),
        (&PSETID_MEETING, 0x0002) => Some("Where"),
        (&PSETID_MEETING, 0x0005) => Some("IsRecurring"),
        (&PSETID_MEETING, 0x0006) => Some("RequiredAttendees"),
        (&PSETID_MEETING, 0x0007) => Some("OptionalAttendees"),
        (&PSETID_MEETING, 0x0008) => Some("ResourceAttendees"),
        (&PSETID_MEETING, 0x000C) => Some("TimeZone"),
        (&PSETID_MEETING, 0x0014) => Some("StartTime"),
        (&PSETID_MEETING, 0x0015) => Some("EndTime"),
        (&PSETID_MEETING, 0x0018) => Some("MeetingType"),
        _ => None,
    }
}

fn parse_substitution_stream_name(name: &str) -> Option<(u16, u16)> {
    let suffix = name.strip_prefix("__substg1.0_")?;
    if suffix.len() < 8 {
        return None;
    }
    Some((
        u16::from_str_radix(&suffix[0..4], 16).ok()?,
        u16::from_str_radix(&suffix[4..8], 16).ok()?,
    ))
}

fn decode_property_value(property_type: u16, bytes: &[u8]) -> Option<CalendarPropertyValue> {
    match property_type {
        0x0003 => Some(CalendarPropertyValue::Integer(i32::from_le_bytes(
            bytes.get(..4)?.try_into().ok()?,
        ) as i64)),
        0x000B => Some(CalendarPropertyValue::Boolean(
            u16::from_le_bytes(bytes.get(..2)?.try_into().ok()?) != 0,
        )),
        0x001E => Some(CalendarPropertyValue::Text(decode_windows_1252(bytes))),
        0x001F => Some(CalendarPropertyValue::Text(decode_utf16le(bytes))),
        0x0040 => filetime_to_iso(bytes).map(CalendarPropertyValue::Time),
        0x0102 => Some(CalendarPropertyValue::Binary(bytes.len())),
        _ => None,
    }
}

fn insert_property(
    properties: &mut HashMap<String, CalendarProperty>,
    definition: &NamedPropertyDefinition,
    property_type: u16,
    value: CalendarPropertyValue,
    source: &str,
) {
    properties
        .entry(definition.name.clone())
        .or_insert_with(|| CalendarProperty {
            name: definition.name.clone(),
            property_id: Some(definition.property_id),
            property_type: format!("0x{property_type:04X}"),
            value,
            source: source.to_string(),
        });
}

fn parser_property_value(name: &str, value: &str) -> CalendarPropertyValue {
    if matches!(
        name,
        "ReminderSet" | "Recurring" | "AppointmentSubType" | "IsRecurring"
    ) {
        if let Some(value) = parse_bool(value) {
            return CalendarPropertyValue::Boolean(value);
        }
    }
    if matches!(
        name,
        "ReminderDelta"
            | "BusyStatus"
            | "AppointmentDuration"
            | "AppointmentStateFlags"
            | "ResponseStatus"
    ) {
        if let Ok(value) = value.trim().parse::<i64>() {
            return CalendarPropertyValue::Integer(value);
        }
    }
    if matches!(
        name,
        "ReminderTime"
            | "CommonStart"
            | "CommonEnd"
            | "AppointmentStartWhole"
            | "AppointmentEndWhole"
            | "StartTime"
            | "EndTime"
    ) {
        return CalendarPropertyValue::Time(value.trim().to_string());
    }
    if matches!(name, "AppointmentRecur" | "TimeZoneStruct" | "TimeZone") {
        let trimmed = value.trim();
        let byte_length = trimmed.len() / 2;
        return CalendarPropertyValue::Binary(byte_length);
    }
    CalendarPropertyValue::Text(value.trim().to_string())
}

fn property_diagnostic(property: &CalendarProperty) -> CalendarPropertyDiagnostic {
    CalendarPropertyDiagnostic {
        name: property.name.clone(),
        property_id: property
            .property_id
            .map(|id| format!("0x{id:04X}"))
            .unwrap_or_else(|| "unavailable".to_string()),
        property_type: property.property_type.clone(),
        value: property_value_label(&property.value),
        source: property.source.clone(),
    }
}

fn property_value_label(value: &CalendarPropertyValue) -> String {
    match value {
        CalendarPropertyValue::Text(value) | CalendarPropertyValue::Time(value) => {
            value_or_none(value)
        }
        CalendarPropertyValue::Integer(value) => value.to_string(),
        CalendarPropertyValue::Boolean(value) => value.to_string(),
        CalendarPropertyValue::Binary(bytes) => format!("binary ({bytes} bytes)"),
    }
}

fn first_property_text(properties: &HashMap<String, CalendarProperty>, names: &[&str]) -> String {
    names
        .iter()
        .find_map(|name| properties.get(*name).and_then(property_text))
        .unwrap_or_default()
}

fn property_text(property: &CalendarProperty) -> Option<String> {
    match &property.value {
        CalendarPropertyValue::Text(value) | CalendarPropertyValue::Time(value)
            if !value.trim().is_empty() =>
        {
            Some(value.trim().to_string())
        }
        _ => None,
    }
}

fn property_bool(properties: &HashMap<String, CalendarProperty>, names: &[&str]) -> Option<bool> {
    names.iter().find_map(|name| {
        let property = properties.get(*name)?;
        match &property.value {
            CalendarPropertyValue::Boolean(value) => Some(*value),
            CalendarPropertyValue::Integer(value) => Some(*value != 0),
            CalendarPropertyValue::Text(value) => parse_bool(value),
            _ => None,
        }
    })
}

fn property_integer(properties: &HashMap<String, CalendarProperty>, names: &[&str]) -> Option<i64> {
    names.iter().find_map(|name| {
        let property = properties.get(*name)?;
        match &property.value {
            CalendarPropertyValue::Integer(value) => Some(*value),
            CalendarPropertyValue::Text(value) => value.trim().parse().ok(),
            _ => None,
        }
    })
}

fn property_binary_len(
    properties: &HashMap<String, CalendarProperty>,
    name: &str,
) -> Option<usize> {
    match &properties.get(name)?.value {
        CalendarPropertyValue::Binary(bytes) if *bytes > 0 => Some(*bytes),
        _ => None,
    }
}

fn attendee_source(
    properties: &HashMap<String, CalendarProperty>,
    property_name: &str,
    fallback: &str,
    fallback_label: &str,
) -> String {
    if properties
        .get(property_name)
        .and_then(property_text)
        .is_some()
    {
        format!("Named property {property_name}")
    } else if !fallback.trim().is_empty() {
        fallback_label.to_string()
    } else {
        String::new()
    }
}

fn reminder_summary(properties: &HashMap<String, CalendarProperty>) -> String {
    let enabled = property_bool(properties, &["ReminderSet"]);
    if enabled == Some(false) {
        return "Off".to_string();
    }
    if let Some(minutes) = property_integer(properties, &["ReminderDelta"]) {
        return if enabled == Some(true) {
            format!("{minutes} minutes before")
        } else {
            format!("{minutes} minutes before (reminder state unavailable)")
        };
    }
    let time = first_property_text(properties, &["ReminderTime"]);
    if !time.is_empty() {
        return format!("Reminder time: {time}");
    }
    if enabled == Some(true) {
        "On".to_string()
    } else {
        String::new()
    }
}

fn response_status_label(value: i64) -> String {
    match value {
        0 => "None",
        1 => "Organizer",
        2 => "Tentative",
        3 => "Accepted",
        4 => "Declined",
        5 => "Not responded",
        _ => "Unknown response status",
    }
    .to_string()
}

fn busy_status_label(value: i64) -> String {
    match value {
        0 => "Free",
        1 => "Tentative",
        2 => "Busy",
        3 => "Out of Office",
        4 => "Working Elsewhere",
        _ => "Unknown availability",
    }
    .to_string()
}

fn sensitivity_label(value: u32) -> &'static str {
    match value {
        1 => "Personal",
        2 => "Private",
        3 => "Confidential",
        _ => "Normal",
    }
}

fn validate_calendar_date(label: &str, value: &str, warnings: &mut Vec<String>) {
    if !value.trim().is_empty() && DateTime::parse_from_rfc3339(value.trim()).is_err() {
        warnings.push(format!(
            "Calendar {label} value was preserved but could not be interpreted as an ISO date: {value}"
        ));
    }
}

fn is_calendar_property_name(name: &str) -> bool {
    matches!(
        name,
        "ReminderDelta"
            | "ReminderTime"
            | "ReminderSet"
            | "CommonStart"
            | "CommonEnd"
            | "BusyStatus"
            | "Location"
            | "AppointmentStartWhole"
            | "AppointmentEndWhole"
            | "AppointmentDuration"
            | "AppointmentRecur"
            | "AppointmentStateFlags"
            | "ResponseStatus"
            | "Recurring"
            | "AppointmentSubType"
            | "TimeZoneDescription"
            | "TimeZoneStruct"
            | "AllAttendeesString"
            | "Where"
            | "IsRecurring"
            | "RequiredAttendees"
            | "OptionalAttendees"
            | "ResourceAttendees"
            | "TimeZone"
            | "StartTime"
            | "EndTime"
            | "MeetingType"
            | "Keywords"
            | "Categories"
    )
}

fn known_property_id(name: &str) -> Option<u16> {
    match name {
        "ReminderDelta" => Some(0x8501),
        "ReminderTime" => Some(0x8502),
        "ReminderSet" => Some(0x8503),
        "CommonStart" => Some(0x8516),
        "CommonEnd" => Some(0x8517),
        "BusyStatus" => Some(0x8205),
        "Location" => Some(0x8208),
        "AppointmentStartWhole" => Some(0x820D),
        "AppointmentEndWhole" => Some(0x820E),
        "AppointmentDuration" => Some(0x8213),
        "AppointmentRecur" => Some(0x8215),
        "AppointmentStateFlags" => Some(0x8216),
        "ResponseStatus" => Some(0x8217),
        "Recurring" => Some(0x8223),
        "AppointmentSubType" => Some(0x8231),
        "TimeZoneDescription" => Some(0x8234),
        "TimeZoneStruct" => Some(0x8235),
        "AllAttendeesString" => Some(0x8256),
        "Where" => Some(0x0002),
        "IsRecurring" => Some(0x0005),
        "RequiredAttendees" => Some(0x0006),
        "OptionalAttendees" => Some(0x0007),
        "ResourceAttendees" => Some(0x0008),
        "TimeZone" => Some(0x000C),
        "StartTime" => Some(0x0014),
        "EndTime" => Some(0x0015),
        "MeetingType" => Some(0x0018),
        _ => None,
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn filetime_to_iso(bytes: &[u8]) -> Option<String> {
    let ticks = u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?);
    let seconds_since_1601 = ticks / 10_000_000;
    let nanos = ((ticks % 10_000_000) * 100) as u32;
    let unix_seconds = i64::try_from(seconds_since_1601).ok()? - 11_644_473_600;
    DateTime::<Utc>::from_timestamp(unix_seconds, nanos).map(|date| date.to_rfc3339())
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let units = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&units)
        .trim_end_matches('\0')
        .to_string()
}

fn decode_windows_1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| match byte {
            0x80 => '€',
            0x82 => '‚',
            0x83 => 'ƒ',
            0x84 => '„',
            0x85 => '…',
            0x86 => '†',
            0x87 => '‡',
            0x88 => 'ˆ',
            0x89 => '‰',
            0x8A => 'Š',
            0x8B => '‹',
            0x8C => 'Œ',
            0x8E => 'Ž',
            0x91 => '‘',
            0x92 => '’',
            0x93 => '“',
            0x94 => '”',
            0x95 => '•',
            0x96 => '–',
            0x97 => '—',
            0x98 => '˜',
            0x99 => '™',
            0x9A => 'š',
            0x9B => '›',
            0x9C => 'œ',
            0x9E => 'ž',
            0x9F => 'Ÿ',
            value => char::from(*value),
        })
        .collect()
}

fn format_person(person: &msg_parser::Person) -> String {
    if person.name.trim().is_empty() {
        person.email.trim().to_string()
    } else if person.email.trim().is_empty() || person.name.trim() == person.email.trim() {
        person.name.trim().to_string()
    } else {
        format!("{} <{}>", person.name.trim(), person.email.trim())
    }
}

fn format_people(people: &[msg_parser::Person]) -> String {
    people
        .iter()
        .map(format_person)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

fn value_or_none(value: &str) -> String {
    if value.trim().is_empty() {
        "(none)".to_string()
    } else {
        value.trim().to_string()
    }
}

fn option_bool_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn property(name: &str, value: CalendarPropertyValue) -> CalendarProperty {
        CalendarProperty {
            name: name.to_string(),
            property_id: known_property_id(name),
            property_type: "test".to_string(),
            value,
            source: "test fixture".to_string(),
        }
    }

    fn test_input(message_class: &str) -> CalendarBuildInput {
        CalendarBuildInput {
            message_class: message_class.to_string(),
            organizer: "Organizer <organizer@example.com>".to_string(),
            to: "Required Person <required@example.com>".to_string(),
            cc: "Optional Person <optional@example.com>".to_string(),
            sensitivity: 0,
            creation_time: "2026-01-01T12:00:00+00:00".to_string(),
            modification_time: "2026-01-02T12:00:00+00:00".to_string(),
            properties: HashMap::new(),
            parse_warnings: Vec::new(),
        }
    }

    #[test]
    fn recognizes_supported_calendar_classes_and_leaves_notes_alone() {
        for (class, expected) in [
            ("IPM.Appointment", "Appointment"),
            ("IPM.Appointment.Custom", "Appointment"),
            ("IPM.Schedule.Meeting.Request", "Meeting Request"),
            ("IPM.Schedule.Meeting.Resp.Pos", "Meeting Accepted"),
            ("IPM.Schedule.Meeting.Resp.Neg", "Meeting Declined"),
            ("IPM.Schedule.Meeting.Resp.Tent", "Meeting Tentative"),
            ("IPM.Schedule.Meeting.Canceled", "Meeting Cancellation"),
        ] {
            assert_eq!(calendar_item_type(class), Some(expected));
        }
        assert_eq!(calendar_item_type("IPM.Note"), None);
        assert_eq!(calendar_item_type("IPM.Task"), None);
    }

    #[test]
    fn appointment_fields_preserve_raw_dates_and_attendee_sources() {
        let mut input = test_input("IPM.Schedule.Meeting.Request");
        input.properties.insert(
            "AppointmentStartWhole".to_string(),
            property(
                "AppointmentStartWhole",
                CalendarPropertyValue::Time("2026-07-20T14:00:00+00:00".to_string()),
            ),
        );
        input.properties.insert(
            "AppointmentEndWhole".to_string(),
            property(
                "AppointmentEndWhole",
                CalendarPropertyValue::Time("2026-07-20T15:00:00+00:00".to_string()),
            ),
        );
        input.properties.insert(
            "Location".to_string(),
            property(
                "Location",
                CalendarPropertyValue::Text("Conference Room A".to_string()),
            ),
        );
        input.properties.insert(
            "TimeZoneDescription".to_string(),
            property(
                "TimeZoneDescription",
                CalendarPropertyValue::Text("Eastern Standard Time".to_string()),
            ),
        );
        input.properties.insert(
            "ResponseStatus".to_string(),
            property("ResponseStatus", CalendarPropertyValue::Integer(5)),
        );
        let details = build_calendar_details("Meeting Request", input);

        assert_eq!(details.start_raw, "2026-07-20T14:00:00+00:00");
        assert_eq!(details.end_raw, "2026-07-20T15:00:00+00:00");
        assert_eq!(details.location, "Conference Room A");
        assert_eq!(details.time_zone, "Eastern Standard Time");
        assert!(!details.time_zone_uncertain);
        assert_eq!(
            details.required_attendees_source,
            "MSG To recipient table fallback"
        );
        assert_eq!(details.response_status, "Not responded");
    }

    #[test]
    fn all_day_and_recurrence_are_reported_without_guessing_pattern() {
        let mut input = test_input("IPM.Appointment");
        input.properties.insert(
            "AppointmentSubType".to_string(),
            property("AppointmentSubType", CalendarPropertyValue::Boolean(true)),
        );
        input.properties.insert(
            "AppointmentRecur".to_string(),
            property("AppointmentRecur", CalendarPropertyValue::Binary(96)),
        );
        let details = build_calendar_details("Appointment", input);

        assert_eq!(details.all_day, Some(true));
        assert!(details.recurrence_available);
        assert_eq!(details.recurrence_summary, "Recurring meeting");
        assert_eq!(
            details.recurrence_raw_summary,
            "Outlook recurrence pattern present (96 bytes)"
        );
        assert!(!details.unsupported_properties.is_empty());
    }

    #[test]
    fn malformed_calendar_dates_warn_without_dropping_value() {
        let mut input = test_input("IPM.Schedule.Meeting.Resp.Neg");
        input.properties.insert(
            "AppointmentStartWhole".to_string(),
            property(
                "AppointmentStartWhole",
                CalendarPropertyValue::Time("not-a-date".to_string()),
            ),
        );
        let details = build_calendar_details("Meeting Declined", input);
        assert_eq!(details.start_raw, "not-a-date");
        assert!(details
            .parse_warnings
            .iter()
            .any(|warning| warning.contains("could not be interpreted")));
    }

    #[test]
    fn meeting_response_does_not_mislabel_sender_or_recipients_as_attendees() {
        let input = test_input("IPM.Schedule.Meeting.Resp.Pos");
        let details = build_calendar_details("Meeting Accepted", input);

        assert_eq!(details.organizer, "Required Person <required@example.com>");
        assert_eq!(
            details.organizer_source,
            "MSG To recipient table for meeting response"
        );
        assert!(details.required_attendees.is_empty());
        assert!(details.optional_attendees.is_empty());
        assert_eq!(details.meeting_status, "Meeting Accepted");
    }

    #[test]
    fn nameid_parser_resolves_meeting_attendee_property() {
        let guid_stream = PSETID_MEETING.to_vec();
        let name_id = 0x0006u32.to_le_bytes();
        let packed = ((3u32 << 1) | (7u32 << 16)).to_le_bytes();
        let mut entry_stream = Vec::new();
        entry_stream.extend(name_id);
        entry_stream.extend(packed);

        let definitions = parse_named_property_definitions(&guid_stream, &entry_stream, &[]);
        let definition = definitions
            .get(&0x8007)
            .expect("meeting property should resolve");
        assert_eq!(definition.name, "RequiredAttendees");
    }
}
