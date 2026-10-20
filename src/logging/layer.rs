//! The tracing layer that writes warnings and errors to the log file, with the `request_id` of the
//! tool call they happened in (requirements R-OBS-1 and R-ERR-2).

use std::{fmt, sync::Arc};

use serde_json::{Map, Value};
use tracing::{
    Event,
    Level,
    Metadata,
    Subscriber,
    field::{Field, Visit},
    span,
};
use tracing_subscriber::{layer::Context, registry::LookupSpan};

use super::{CALL_SPAN, REQUEST_ID_FIELD, file::LogFile};

/// Writes each event it sees as a line of JSON.
pub(super) struct FileLayer {
    pub(super) file: Arc<LogFile>,
}

/// The `request_id` of a [`CALL_SPAN`], kept in the span's extensions.
struct RequestId(Box<str>);

/// Whether the file layer sees `metadata`: the spans of tool calls, for their `request_id`, and
/// events of the level warn and above. Nothing else filters it (requirement R-DIST-3).
pub(super) fn enabled(metadata: &Metadata<'_>) -> bool {
    if metadata.is_span() {
        metadata.name() == CALL_SPAN
    } else {
        *metadata.level() <= Level::WARN
    }
}

impl<S> tracing_subscriber::Layer<S> for FileLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attributes: &span::Attributes<'_>,
        id: &span::Id,
        context: Context<'_, S>,
    ) {
        if attributes.metadata().name() != CALL_SPAN {
            return;
        }
        let mut fields = Map::new();
        attributes.record(&mut Fields(&mut fields));
        if let (Some(Value::String(request_id)), Some(span)) =
            (fields.remove(REQUEST_ID_FIELD), context.span(id))
        {
            span.extensions_mut().insert(RequestId(request_id.into()));
        }
    }

    fn on_event(&self, event: &Event<'_>, context: Context<'_, S>) {
        let mut fields = Map::new();
        event.record(&mut Fields(&mut fields));
        let request_id = context.event_scope(event).and_then(|scope| {
            scope
                .into_iter()
                .find_map(|span| span.extensions().get::<RequestId>().map(|id| id.0.clone()))
        });
        let metadata = event.metadata();
        let mut line = Map::new();
        line.insert("ts".to_owned(), super::file::now().into());
        line.insert("level".to_owned(), metadata.level().as_str().into());
        if let Some(request_id) = request_id {
            line.insert(REQUEST_ID_FIELD.to_owned(), String::from(request_id).into());
        }
        line.insert("target".to_owned(), metadata.target().into());
        if let Some(message) = fields.remove("message") {
            line.insert("message".to_owned(), message);
        }
        if !fields.is_empty() {
            line.insert("fields".to_owned(), fields.into());
        }
        self.file.write_line(&Value::Object(line).to_string());
    }
}

/// Collects the fields of a span or an event as JSON.
struct Fields<'a>(&'a mut Map<String, Value>);

impl Visit for Fields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0
            .insert(field.name().to_owned(), format!("{value:?}").into());
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        let mut text = value.to_string();
        for source in std::iter::successors(value.source(), |&source| source.source()) {
            text.push_str(": ");
            text.push_str(&source.to_string());
        }
        self.0.insert(field.name().to_owned(), text.into());
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tracing_subscriber::{Layer as _, filter::filter_fn, layer::SubscriberExt as _};

    use super::{super::file::Limits, *};

    #[test]
    fn warnings_in_a_call_carry_its_request_id_and_nothing_else_is_written() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let file = Arc::new(
            LogFile::open(temp.path(), Limits {
                file_max_size: 1_000_000,
                retention: Duration::from_hours(24),
                total_max_size: 1_000_000,
            })
            .expect("opened"),
        );
        let subscriber = tracing_subscriber::registry().with(
            FileLayer {
                file: Arc::clone(&file),
            }
            .with_filter(filter_fn(enabled)),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!(CALL_SPAN, request_id = %"abc-1", tool = "get_item").in_scope(
                || {
                    tracing::info!("not written");
                    tracing::info_span!("inner").in_scope(|| {
                        tracing::warn!(krate = "tokio", count = 3_u64, "a warning");
                    });
                },
            );
            tracing::error!("outside a call");
        });
        let text = std::fs::read_to_string(file.path()).expect("readable");
        let lines: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON"))
            .collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert_eq!(lines[0]["level"], "WARN");
        assert_eq!(lines[0]["request_id"], "abc-1");
        assert_eq!(lines[0]["message"], "a warning");
        assert_eq!(
            lines[0]["fields"],
            serde_json::json!({ "krate": "tokio", "count": 3 })
        );
        assert_eq!(lines[1]["level"], "ERROR");
        assert!(lines[1].get("request_id").is_none(), "{text}");
    }
}
