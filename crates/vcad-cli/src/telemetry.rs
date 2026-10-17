//! OpenTelemetry traces for long CLI runs (the autorouter above all), so a slow
//! route shows where its time went. Off unless `OTEL_EXPORTER_OTLP_ENDPOINT`
//! is set; spans go out over OTLP/HTTP to any collector (Jaeger, Tempo, ...):
//!
//!     docker run -d -p 16686:16686 -p 4318:4318 jaegertracing/all-in-one
//!     OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 vcad route ...
//!
//! then open http://localhost:16686, service `vcad`.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Flushes and shuts the exporter down when dropped (end of `main`).
pub struct Guard(Option<SdkTracerProvider>);

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = p.shutdown();
        }
    }
}

pub fn init() -> Guard {
    if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_none() {
        return Guard(None);
    }
    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("warning: OTLP exporter: {e}; tracing off");
            return Guard(None);
        }
    };
    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name("vcad")
        .build();
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();
    let tracer = provider.tracer("vcad");
    tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .init();
    Guard(Some(provider))
}
