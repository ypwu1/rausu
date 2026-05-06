//! OpenTelemetry tracing wiring for the gateway.
//!
//! ## Privacy
//!
//! Spans intentionally never carry prompt or response bodies — only safe
//! metadata such as HTTP method, route, status, provider name, model, and
//! token usage counts.  All call-sites that record attributes should follow
//! that contract.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use axum::extract::Request;
use axum::http::{HeaderMap, Response};
use axum::middleware::Next;
use opentelemetry::propagation::Extractor;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

use crate::config::schema::OtelConfig;

/// Default service name used when no override is supplied.
pub const DEFAULT_SERVICE_NAME: &str = "rausu";

/// Default OTLP HTTP endpoint (the OpenTelemetry Collector).
pub const DEFAULT_OTLP_HTTP_ENDPOINT: &str = "http://localhost:4318/v1/traces";

/// Holds the active tracer provider so it can be flushed on shutdown.
///
/// The provider is set once at startup and read on shutdown.  We use a
/// process-global `OnceLock` because the OTel SDK itself stores a global
/// tracer provider and we want to mirror that here to ease shutdown.
static TRACER_PROVIDER: OnceLock<SdkTracerProvider> = OnceLock::new();

/// Effective OTel configuration after applying environment overrides.
#[derive(Debug, Clone)]
struct EffectiveOtelConfig {
    enabled: bool,
    exporter: String,
    endpoint: String,
    service_name: String,
    headers: Vec<(String, String)>,
}

impl EffectiveOtelConfig {
    fn from(config: &OtelConfig) -> Self {
        let enabled = read_bool_env("RAUSU_OTEL_ENABLED")
            .or_else(|| read_bool_env("OTEL_SDK_DISABLED").map(|v| !v))
            .unwrap_or(config.enabled);

        let exporter = read_env("RAUSU_OTEL_EXPORTER")
            .or_else(|| read_env("OTEL_TRACES_EXPORTER"))
            .or_else(|| config.exporter.clone())
            .unwrap_or_else(|| "otlp_http".to_string());

        let endpoint = read_env("RAUSU_OTEL_ENDPOINT")
            .or_else(|| read_env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"))
            .or_else(|| read_env("OTEL_EXPORTER_OTLP_ENDPOINT"))
            .or_else(|| config.endpoint.clone())
            .unwrap_or_else(|| DEFAULT_OTLP_HTTP_ENDPOINT.to_string());

        let service_name = read_env("RAUSU_OTEL_SERVICE_NAME")
            .or_else(|| read_env("OTEL_SERVICE_NAME"))
            .or_else(|| config.service_name.clone())
            .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_string());

        let mut headers: Vec<(String, String)> = config
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if let Some(env_headers) =
            read_env("RAUSU_OTEL_HEADERS").or_else(|| read_env("OTEL_EXPORTER_OTLP_HEADERS"))
        {
            for (k, v) in parse_header_list(&env_headers) {
                headers.retain(|(existing, _)| !existing.eq_ignore_ascii_case(&k));
                headers.push((k, v));
            }
        }

        Self {
            enabled,
            exporter,
            endpoint,
            service_name,
            headers,
        }
    }
}

fn read_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn read_bool_env(name: &str) -> Option<bool> {
    read_env(name).and_then(|v| match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    })
}

/// Parse a comma-separated `k=v,k2=v2` header list (W3C OTLP convention).
pub(crate) fn parse_header_list(s: &str) -> Vec<(String, String)> {
    s.split(',')
        .filter_map(|pair| {
            let pair = pair.trim();
            if pair.is_empty() {
                return None;
            }
            let (k, v) = pair.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// Initialise the global tracing subscriber.
///
/// When `otel.enabled` is false (the default), only the existing fmt
/// subscriber is registered — i.e. behaviour identical to the pre-OTel build.
/// When enabled, an OTLP exporter is configured and an
/// [`tracing_opentelemetry`] layer is composed alongside the fmt layer.
///
/// Returns a [`TracerGuard`] that flushes the provider on drop (so the
/// shutdown path flushes any pending spans even on error/panic).
pub fn init_tracing(
    log_level: &str,
    use_json: bool,
    otel: Option<&OtelConfig>,
) -> Result<TracerGuard> {
    let env_filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));

    let fmt_layer: Box<dyn Layer<Registry> + Send + Sync> = if use_json {
        Box::new(
            tracing_subscriber::fmt::layer()
                .json()
                .with_filter(env_filter()),
        )
    } else {
        Box::new(
            tracing_subscriber::fmt::layer()
                .pretty()
                .with_filter(env_filter()),
        )
    };

    let otel_cfg = otel.map(EffectiveOtelConfig::from);
    let enabled = matches!(&otel_cfg, Some(c) if c.enabled);

    if let Some(cfg) = otel_cfg.filter(|c| c.enabled) {
        let provider =
            build_tracer_provider(&cfg).context("Failed to build OpenTelemetry tracer provider")?;
        let tracer = provider.tracer(cfg.service_name.clone());
        // Install the W3C trace context propagator so incoming
        // `traceparent`/`tracestate`/`baggage` headers continue the trace.
        global::set_text_map_propagator(TraceContextPropagator::new());
        global::set_tracer_provider(provider.clone());
        // Stash for shutdown flush.
        let _ = TRACER_PROVIDER.set(provider);

        let otel_layer = tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_filter(env_filter());

        Registry::default()
            .with(fmt_layer)
            .with(otel_layer)
            .try_init()
            .map_err(|e| anyhow!("Failed to install tracing subscriber: {e}"))?;
    } else {
        Registry::default()
            .with(fmt_layer)
            .try_init()
            .map_err(|e| anyhow!("Failed to install tracing subscriber: {e}"))?;
    }

    Ok(TracerGuard { enabled })
}

/// Construct an OTLP `SdkTracerProvider` from effective config.
fn build_tracer_provider(cfg: &EffectiveOtelConfig) -> Result<SdkTracerProvider> {
    if cfg.exporter != "otlp_http" && cfg.exporter != "otlp" {
        return Err(anyhow!(
            "Unsupported OTel exporter '{}': only 'otlp_http' is supported in this build",
            cfg.exporter
        ));
    }

    let mut builder = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(cfg.endpoint.clone())
        .with_protocol(Protocol::HttpBinary)
        .with_timeout(Duration::from_secs(10));

    if !cfg.headers.is_empty() {
        let map: std::collections::HashMap<String, String> = cfg.headers.iter().cloned().collect();
        builder = builder.with_headers(map);
    }

    let exporter = builder
        .build()
        .context("Failed to build OTLP HTTP span exporter")?;

    let resource = Resource::builder()
        .with_service_name(cfg.service_name.clone())
        .with_attribute(KeyValue::new(
            "service.version",
            env!("CARGO_PKG_VERSION").to_string(),
        ))
        .build();

    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build())
}

/// Guard returned by [`init_tracing`].  Flushes pending spans on drop.
#[derive(Debug)]
pub struct TracerGuard {
    enabled: bool,
}

impl TracerGuard {
    /// Whether OTel export was actually enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Synchronously shut down the tracer provider, flushing any pending spans.
    ///
    /// Safe to call multiple times — subsequent calls are no-ops.
    pub fn shutdown(&self) {
        if let Some(provider) = TRACER_PROVIDER.get() {
            if let Err(e) = provider.shutdown() {
                eprintln!("OpenTelemetry tracer shutdown error: {e}");
            }
        }
    }
}

impl Drop for TracerGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// `Extractor` adapter for `axum`/`http` `HeaderMap`.
struct HeaderMapExtractor<'a>(&'a HeaderMap);

impl<'a> Extractor for HeaderMapExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

/// Extract a W3C trace context from request headers, returning the
/// remote-parent OpenTelemetry [`Context`](opentelemetry::Context).
pub fn extract_context(headers: &HeaderMap) -> opentelemetry::Context {
    global::get_text_map_propagator(|p| p.extract(&HeaderMapExtractor(headers)))
}

/// Axum middleware that wraps every request in a `Received Proxy Server
/// Request` span and links it to any incoming W3C trace context.
///
/// The span carries only safe metadata — never the request body.
pub async fn trace_request(req: Request, next: Next) -> Response<axum::body::Body> {
    use tracing::Instrument;

    let method = req.method().clone();
    let route = req.uri().path().to_string();

    let span = tracing::info_span!(
        "Received Proxy Server Request",
        otel.name = "Received Proxy Server Request",
        http.method = %method,
        http.route = %route,
        http.status_code = tracing::field::Empty,
        error.type = tracing::field::Empty,
    );

    // Continue any incoming trace context.  `set_parent` returns a `Result`
    // when the global tracer is not initialised — that's fine, we only
    // attach context when it succeeds.
    let parent = extract_context(req.headers());
    let _ = span.set_parent(parent);

    let response_span = span.clone();
    let response = next.run(req).instrument(span).await;

    response_span.record("http.status_code", response.status().as_u16());

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_header_list_empty() {
        assert!(parse_header_list("").is_empty());
        assert!(parse_header_list("   ").is_empty());
    }

    #[test]
    fn parse_header_list_single() {
        let h = parse_header_list("api-key=abc");
        assert_eq!(h, vec![("api-key".to_string(), "abc".to_string())]);
    }

    #[test]
    fn parse_header_list_multiple_with_spaces() {
        let h = parse_header_list("a=1, b=2,c=3");
        assert_eq!(
            h,
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string()),
                ("c".to_string(), "3".to_string()),
            ]
        );
    }

    #[test]
    fn parse_header_list_skips_malformed() {
        let h = parse_header_list("good=ok,malformed,also=fine");
        assert_eq!(
            h,
            vec![
                ("good".to_string(), "ok".to_string()),
                ("also".to_string(), "fine".to_string()),
            ]
        );
    }

    #[test]
    fn effective_config_uses_defaults() {
        // Clear any environment overrides that could leak from the test runner.
        for key in [
            "RAUSU_OTEL_ENABLED",
            "RAUSU_OTEL_EXPORTER",
            "RAUSU_OTEL_ENDPOINT",
            "RAUSU_OTEL_SERVICE_NAME",
            "RAUSU_OTEL_HEADERS",
            "OTEL_TRACES_EXPORTER",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_SERVICE_NAME",
            "OTEL_EXPORTER_OTLP_HEADERS",
            "OTEL_SDK_DISABLED",
        ] {
            std::env::remove_var(key);
        }

        let cfg = OtelConfig {
            enabled: false,
            exporter: None,
            endpoint: None,
            service_name: None,
            headers: std::collections::BTreeMap::new(),
        };
        let eff = EffectiveOtelConfig::from(&cfg);
        assert!(!eff.enabled);
        assert_eq!(eff.exporter, "otlp_http");
        assert_eq!(eff.endpoint, DEFAULT_OTLP_HTTP_ENDPOINT);
        assert_eq!(eff.service_name, DEFAULT_SERVICE_NAME);
        assert!(eff.headers.is_empty());
    }

    #[test]
    fn extract_context_no_headers_returns_invalid() {
        // Without headers we still get a Context — its span context will be invalid.
        let headers = HeaderMap::new();
        let ctx = extract_context(&headers);
        // Just ensure no panic and we got *some* Context.
        let _ = ctx;
    }
}
