use crate::error::Error;
use crate::tracer::Tracer;
use crate::tracer::provider::Otel;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use opentelemetry::global::BoxedTracer;
use std::env;

const PUBLIC_KEY_ENV_VAR: &str = "LANGFUSE_PUBLIC_KEY";
const SECRET_KEY_ENV_VAR: &str = "LANGFUSE_SECRET_KEY";
const BASE_URL_ENV_VAR: &str = "LANGFUSE_BASE_URL";
const LEGACY_BASE_URL_ENV_VAR: &str = "LANGFUSE_HOST";
const TRACES_PATH: &str = "/api/public/otel/v1/traces";
const INGESTION_VERSION_HEADER: &str = "x-langfuse-ingestion-version";
const INGESTION_VERSION: &str = "4";

#[derive(Debug)]
pub struct Langfuse {
    otel: Otel,
}

#[derive(Debug, Clone)]
pub struct LangfuseBuilder {
    public_key: String,
    secret_key: String,
    base_url: String,
    service_name: Option<String>,
}

impl Langfuse {
    pub fn from_env() -> Result<Self, Error> {
        let public_key =
            env::var(PUBLIC_KEY_ENV_VAR).map_err(|_| Error::MissingEnvVar(PUBLIC_KEY_ENV_VAR))?;
        let secret_key =
            env::var(SECRET_KEY_ENV_VAR).map_err(|_| Error::MissingEnvVar(SECRET_KEY_ENV_VAR))?;
        let base_url = base_url_from_env().ok_or(Error::MissingEnvVar(BASE_URL_ENV_VAR))?;
        Self::builder(public_key, secret_key, base_url).build()
    }

    pub fn builder(
        public_key: impl Into<String>,
        secret_key: impl Into<String>,
        base_url: impl Into<String>,
    ) -> LangfuseBuilder {
        LangfuseBuilder {
            public_key: public_key.into(),
            secret_key: secret_key.into(),
            base_url: base_url.into(),
            service_name: None,
        }
    }
}

impl Tracer for Langfuse {
    fn tracer(&self) -> BoxedTracer {
        self.otel.tracer()
    }

    fn shutdown(&self) -> Result<(), Error> {
        self.otel.shutdown()
    }
}

impl LangfuseBuilder {
    pub fn service_name(mut self, service_name: impl Into<String>) -> Self {
        self.service_name = Some(service_name.into());
        self
    }

    pub fn build(self) -> Result<Langfuse, Error> {
        let endpoint = format!("{}{}", self.base_url.trim_end_matches('/'), TRACES_PATH);
        let credentials = STANDARD.encode(format!("{}:{}", self.public_key, self.secret_key));
        let otel = Otel::builder(endpoint)
            .header("Authorization", format!("Basic {credentials}"))
            .header(INGESTION_VERSION_HEADER, INGESTION_VERSION);
        let otel = match self.service_name {
            Some(service_name) => otel.service_name(service_name),
            None => otel,
        };
        Ok(Langfuse {
            otel: otel.build()?,
        })
    }
}

fn base_url_from_env() -> Option<String> {
    env::var(BASE_URL_ENV_VAR)
        .or_else(|_| env::var(LEGACY_BASE_URL_ENV_VAR))
        .ok()
}
