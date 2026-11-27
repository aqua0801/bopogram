//! The IBus objects that travel inside D-Bus variants, as IBus's public
//! documentation of its serializable types describes them: each is a
//! structure that starts with its type name and a dictionary of attachments.
//!
//!   IBusText        (s a{sv} s v)       name, attachments, text, IBusAttrList
//!   IBusAttrList    (s a{sv} av)        name, attachments, attributes
//!   IBusAttribute   (s a{sv} u u u u)   name, attachments, type, value, start, end
//!   IBusLookupTable (s a{sv} u u b b i av av)
//!       name, attachments, page size, cursor, cursor visible, round,
//!       orientation, candidates (IBusText), labels (IBusText)

use std::collections::HashMap;

use zbus::zvariant::{StructureBuilder, Value};

const ATTR_UNDERLINE: u32 = 1;
const UNDERLINE_SINGLE: u32 = 1;
const ORIENTATION_SYSTEM: i32 = 2;

fn attachments() -> HashMap<String, Value<'static>> {
    HashMap::new()
}

/// A value in a `v` field of a structure. (Elements of an `av` array are
/// written as variants anyway: they go in as they are.)
fn variant(value: Value<'static>) -> Value<'static> {
    Value::Value(Box::new(value))
}

fn structure(builder: StructureBuilder<'static>) -> Value<'static> {
    Value::Structure(builder.build().expect("a structure with fields"))
}

fn attribute(kind: u32, value: u32, start: u32, end: u32) -> Value<'static> {
    structure(
        StructureBuilder::new()
            .add_field("IBusAttribute")
            .add_field(attachments())
            .add_field(kind)
            .add_field(value)
            .add_field(start)
            .add_field(end),
    )
}

/// An IBusText; `underline` draws the whole text underlined (the composition).
pub fn text(s: &str, underline: bool) -> Value<'static> {
    let length = s.chars().count() as u32;
    let attributes: Vec<Value<'static>> = if underline && length > 0 {
        vec![attribute(ATTR_UNDERLINE, UNDERLINE_SINGLE, 0, length)]
    } else {
        Vec::new()
    };
    let list = structure(
        StructureBuilder::new().add_field("IBusAttrList").add_field(attachments()).add_field(attributes),
    );
    structure(
        StructureBuilder::new()
            .add_field("IBusText")
            .add_field(attachments())
            .add_field(s.to_owned())
            .append_field(variant(list)),
    )
}

/// An IBusLookupTable holding one page of candidates.
pub fn lookup_table(candidates: &[String], labels: &[String], cursor: usize, page_size: usize) -> Value<'static> {
    let texts = |items: &[String]| -> Vec<Value<'static>> { items.iter().map(|s| text(s, false)).collect() };
    structure(
        StructureBuilder::new()
            .add_field("IBusLookupTable")
            .add_field(attachments())
            .add_field(page_size.max(candidates.len()) as u32)
            .add_field(cursor as u32)
            .add_field(true)
            .add_field(false)
            .add_field(ORIENTATION_SYSTEM)
            .add_field(texts(candidates))
            .add_field(texts(labels)),
    )
}

/// The string of an IBusText received from IBus (surrounding text).
pub fn text_of(value: &Value<'_>) -> Option<String> {
    match value {
        Value::Value(inner) => text_of(inner),
        Value::Structure(s) => match s.fields().get(2)? {
            Value::Str(text) => Some(text.as_str().to_owned()),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures() {
        assert_eq!(text("我", true).value_signature().to_string(), "(sa{sv}sv)");
        let table = lookup_table(&["在".into(), "再".into()], &["1".into(), "2".into()], 0, 9);
        assert_eq!(table.value_signature().to_string(), "(sa{sv}uubbiavav)");
        assert_eq!(text_of(&text("我在家", false)).as_deref(), Some("我在家"));
    }
}
