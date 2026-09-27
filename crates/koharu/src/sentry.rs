use sentry::{ClientOptions, IntoDsn};
use tracing_subscriber::registry::LookupSpan;

pub fn initialize() -> sentry::ClientInitGuard {
    // Settings → Privacy → "Send crash reports". Read before anything else
    // starts, so a change applies on the next launch. Without a DSN (any
    // non-release build) the client stays disabled either way.
    let dsn = if koharu_app::config::crash_reports_enabled() {
        option_env!("SENTRY_DSN")
    } else {
        None
    };
    sentry::init(ClientOptions {
        dsn: dsn
            .into_dsn()
            .expect("invalid SENTRY_DSN environment variable"),
        release: sentry::release_name!(),
        // No IP addresses, usernames or other personal data.
        send_default_pii: false,
        sample_rate: 0.1,
        auto_session_tracking: true,
        ..Default::default()
    })
}

pub fn tracing_layer<S>() -> impl tracing_subscriber::Layer<S>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
{
    sentry::integrations::tracing::layer()
}
