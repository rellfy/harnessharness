pub mod provider;
pub mod span;

use crate::error::Error;
use opentelemetry::global::BoxedTracer;

pub const TRACER_NAME: &str = "harnessharness";

pub trait Tracer: Send + Sync {
    fn tracer(&self) -> BoxedTracer;

    fn shutdown(&self) -> Result<(), Error>;
}
