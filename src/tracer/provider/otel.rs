use crate::error::Error;
use crate::tracer::TRACER_NAME;
use crate::tracer::Tracer;
use opentelemetry::global::BoxedTracer;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::Protocol;
use opentelemetry_otlp::SpanExporter;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_otlp::WithHttpConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::collections::HashMap;

#[derive(Debug)]
pub struct Otel {
    provider: SdkTracerProvider,
}

#[derive(Debug, Clone)]
pub struct OtelBuilder {
    endpoint: String,
    headers: HashMap<String, String>,
    service_name: String,
}

impl Otel {
    pub fn builder(endpoint: impl Into<String>) -> OtelBuilder {
        OtelBuilder {
            endpoint: endpoint.into(),
            headers: HashMap::new(),
            service_name: TRACER_NAME.to_string(),
        }
    }
}

impl Tracer for Otel {
    fn tracer(&self) -> BoxedTracer {
        BoxedTracer::new(Box::new(self.provider.tracer(TRACER_NAME)))
    }

    fn shutdown(&self) -> Result<(), Error> {
        self.provider
            .shutdown()
            .map_err(|error| Error::Tracer(error.to_string()))
    }
}

impl OtelBuilder {
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn service_name(mut self, service_name: impl Into<String>) -> Self {
        self.service_name = service_name.into();
        self
    }

    pub fn build(self) -> Result<Otel, Error> {
        let exporter = SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(self.endpoint)
            .with_headers(self.headers)
            .build()
            .map_err(|error| Error::Tracer(error.to_string()))?;
        let resource = Resource::builder()
            .with_service_name(self.service_name)
            .build();
        let provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(resource)
            .build();
        Ok(Otel { provider })
    }
}
