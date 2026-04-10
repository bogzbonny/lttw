use {
    opentelemetry::{global, trace::TracerProvider as _, KeyValue},
    opentelemetry_sdk::{
        metrics::{MeterProviderBuilder, PeriodicReader, SdkMeterProvider},
        trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
        Resource,
    },
    opentelemetry_semantic_conventions::{
        attribute::{DEPLOYMENT_ENVIRONMENT_NAME, SERVICE_VERSION},
        SCHEMA_URL,
    },
    std::str::FromStr,
    tracing_core::Level,
    tracing_opentelemetry::{MetricsLayer, OpenTelemetryLayer},
    tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer},
};

// Re-export tracing macros for use throughout the codebase

// This macro provides backward compatibility with the old debug! macro that could
// accept a single variable like debug!(var), which would print "var = value"
#[macro_export]
macro_rules! debug {
    // Single expression (variable) - like the old debug!(var) syntax
    // This expands to a format string with debug formatting
    ($expr:expr) => {{
        tracing::debug!(target: "lttw", "{} = {:?}", stringify!($expr), $expr);
    }};
    // Format-style variadic - like the old debug!("message {}", arg) syntax
    ($($arg:tt)*) => {{
        tracing::debug!($($arg)*);
    }};
}

//#[macro_export]
//macro_rules! info {
//    // Single expression (variable) - like the old debug!(var) syntax
//    // This expands to a format string with debug formatting
//    ($expr:expr) => {{
//        tracing::info!(target: "lttw", "{} = {:?}", stringify!($expr), $expr);
//    }};
//    // Format-style variadic - like the old debug!("message {}", arg) syntax
//    ($($arg:tt)*) => {{
//        tracing::info!($($arg)*);
//    }};
//}

// RUN WITH:
// docker run -d --name jaeger -e COLLECTOR_OTLP_ENABLED=true -p 16686:16686 -p 4317:4317 jaegertracing/all-in-one:latest
//
// Create a Resource that captures information about the entity for which telemetry is recorded.
fn resource() -> Resource {
    Resource::builder()
        .with_service_name("lttw")
        .with_schema_url(
            [
                KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
                KeyValue::new(DEPLOYMENT_ENVIRONMENT_NAME, "develop"),
            ],
            SCHEMA_URL,
        )
        .build()
}

// Construct MeterProvider for MetricsLayer
fn init_meter_provider() -> SdkMeterProvider {
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_temporality(opentelemetry_sdk::metrics::Temporality::default())
        .build()
        .unwrap();

    let reader = PeriodicReader::builder(exporter)
        .with_interval(std::time::Duration::from_secs(30))
        .build();

    // For debugging in development
    let stdout_reader =
        PeriodicReader::builder(opentelemetry_stdout::MetricExporter::default()).build();

    let meter_provider = MeterProviderBuilder::default()
        .with_resource(resource())
        .with_reader(reader)
        .with_reader(stdout_reader)
        .build();

    global::set_meter_provider(meter_provider.clone());

    meter_provider
}

// Construct TracerProvider for OpenTelemetryLayer
fn init_tracer_provider() -> SdkTracerProvider {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()
        .unwrap();

    SdkTracerProvider::builder()
        // Customize sampling strategy
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
            1.0,
        ))))
        // If export trace to AWS X-Ray, you can use XrayIdGenerator
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(resource())
        .with_batch_exporter(exporter)
        .build()
}

// Initialize tracing-subscriber and return OtelGuard for opentelemetry-related termination processing
pub fn init_tracing_subscriber(log_file: bool, trace_level: String) -> OtelGuard {
    let tracer_provider = init_tracer_provider();
    let meter_provider = init_meter_provider();

    let tracer = tracer_provider.tracer("tracing-otel-subscriber");

    let trace_level = Level::from_str(&trace_level).unwrap_or(Level::DEBUG);

    let mut layers = vec![];

    if log_file {
        // Use tracing_subscriber with a file writer
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("./lttw.log")
            .unwrap();

        // Build the file writer layer for debug logging
        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(file)
            .with_target(false)
            .with_line_number(true)
            .with_file(true)
            .with_thread_names(false)
            .with_ansi(false);

        layers.push(file_layer.boxed());
    }
    tracing_subscriber::registry()
        // The global level filter prevents the exporter network stack
        // from reentering the globally installed OpenTelemetryLayer with
        // its own spans while exporting, as the libraries should not use
        // tracing levels below DEBUG. If the OpenTelemetry layer needs to
        // trace spans and events with higher verbosity levels, consider using
        // per-layer filtering to target the telemetry layer specifically,
        // e.g. by target matching.
        .with(tracing_subscriber::filter::LevelFilter::from_level(
            trace_level,
        ))
        .with(layers)
        .with(MetricsLayer::new(meter_provider.clone()))
        .with(OpenTelemetryLayer::new(tracer))
        .init();

    OtelGuard {
        tracer_provider,
        meter_provider,
    }
}

#[derive(Debug)]
pub struct OtelGuard {
    tracer_provider: SdkTracerProvider,
    meter_provider: SdkMeterProvider,
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        if let Err(err) = self.tracer_provider.shutdown() {
            eprintln!("{err:?}");
        }
        if let Err(err) = self.meter_provider.shutdown() {
            eprintln!("{err:?}");
        }
    }
}

//#[tokio::main]
//async fn main() {
//    let _guard = init_tracing_subscriber();

//    foo().await;
//}

//#[tracing::instrument]
//async fn foo() {
//    tracing::info!(
//        monotonic_counter.foo = 1_u64,
//        key_1 = "bar",
//        key_2 = 10,
//        "handle foo",
//    );
//    tracing::debug!("holla");

//    tracing::info!(histogram.baz = 10, "histogram example",);
//}
