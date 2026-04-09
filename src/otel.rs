use crate::LttwResult;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{fmt, EnvFilter};

pub struct Guard {
    otel: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        tracing::debug!("shutting down tracing");
        if let Some(provider) = self.otel.take() {
            if let Err(e) = provider.shutdown() {
                eprintln!("Failed to shutdown OpenTelemetry: {e:?}");
            }
        }
    }
}

/// Configures tracing
///
/// # Panics
///
/// Panics if setting up tracing fails
pub fn init() -> LttwResult<Guard> {
    let file_appender = tracing_appender::rolling::daily(".", "lttw.log");

    let fmt_layer = fmt::layer().compact().with_writer(file_appender);

    // Logs the file layer will capture
    let mut env_filter_layer = EnvFilter::builder()
        .with_default_directive(LevelFilter::DEBUG.into())
        .from_env_lossy();

    env_filter_layer = env_filter_layer.add_directive("lttw=debug".parse().unwrap());

    let mut layers = vec![fmt_layer.boxed()];

    let guard = init_otel(&mut layers)?;
    let provider_for_guard = Some(guard);

    let registry = tracing_subscriber::registry()
        .with(env_filter_layer)
        .with(layers);
    registry.try_init().unwrap(); // XXX

    Ok(Guard {
        otel: provider_for_guard,
    })
}

fn init_otel<S>(
    layers: &mut Vec<Box<dyn tracing_subscriber::Layer<S> + Send + Sync>>,
) -> LttwResult<opentelemetry_sdk::trace::SdkTracerProvider>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span> + Send + Sync,
{
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithExportConfig as _;
    use opentelemetry_sdk::trace::SdkTracerProvider;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        //.with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
        .with_endpoint("http://localhost:4318")
        .build()
        .unwrap(); // XXX

    let service_name = if let Ok(service_name) = std::env::var("OTEL_SERVICE_NAME") {
        service_name
    } else {
        "lttw".to_string()
    };

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_service_name(service_name)
                .build(),
        )
        .build();

    let tracer = provider.tracer("lttw");
    opentelemetry::global::set_tracer_provider(provider.clone());

    // Create a tracing layer with the configured tracer
    let layer = tracing_opentelemetry::OpenTelemetryLayer::new(tracer);
    layers.push(layer.boxed());

    Ok(provider)
}
