//! Outgoing email. Handlers talk to the [`Mailer`] trait so tests and local development do not
//! need an SMTP server.

use std::sync::{Arc, Mutex};

use axum::async_trait;
use lettre::{
    message::Mailbox, transport::smtp::authentication::Credentials, AsyncSmtpTransport,
    AsyncTransport, Message, Tokio1Executor,
};

// async_trait marks the boxed future `#[must_use]`, which newer clippy reports as a duplicate.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait Mailer: Send + Sync {
    async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), String>;
}

/// Writes messages to the server log instead of sending them. Links in the log are live, so this
/// is for development only.
pub struct LogMailer;

#[async_trait]
impl Mailer for LogMailer {
    async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), String> {
        tracing::info!(
            to,
            subject,
            "email (SMTP not configured, not sent):\n{body}"
        );
        Ok(())
    }
}

/// An email held in memory by [`CaptureMailer`].
#[derive(Debug, Clone)]
pub struct SentMail {
    pub to: String,
    pub subject: String,
    pub body: String,
}

/// Keeps every message so tests can read the links they contain.
#[derive(Default)]
pub struct CaptureMailer {
    sent: Mutex<Vec<SentMail>>,
}

impl CaptureMailer {
    pub fn sent(&self) -> Vec<SentMail> {
        self.sent.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[async_trait]
impl Mailer for CaptureMailer {
    async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), String> {
        self.sent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(SentMail {
                to: to.to_string(),
                subject: subject.to_string(),
                body: body.to_string(),
            });
        Ok(())
    }
}

pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

#[async_trait]
impl Mailer for SmtpMailer {
    async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), String> {
        let message = Message::builder()
            .from(self.from.clone())
            .to(to.parse().map_err(|e| format!("bad recipient: {e}"))?)
            .subject(subject)
            .body(body.to_string())
            .map_err(|e| format!("failed building email: {e}"))?;
        self.transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|e| format!("failed sending email: {e}"))
    }
}

/// Builds the mailer from `SMTP_*` variables; `None` when `SMTP_HOST` is unset.
///
/// `SMTP_TLS` is `starttls` (default, port 587), `tls` (implicit TLS, port 465) or `none`
/// (plaintext, for a local relay only).
pub fn smtp_mailer_from_env() -> Result<Option<SmtpMailer>, String> {
    let Ok(host) = std::env::var("SMTP_HOST") else {
        return Ok(None);
    };
    let from = std::env::var("SMTP_FROM")
        .map_err(|_| "SMTP_FROM is required when SMTP_HOST is set".to_string())?
        .parse::<Mailbox>()
        .map_err(|e| format!("SMTP_FROM is not a valid address: {e}"))?;
    let tls = std::env::var("SMTP_TLS").unwrap_or_else(|_| "starttls".to_string());
    let builder = match tls.as_str() {
        "starttls" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host),
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host),
        "none" => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
            &host,
        )),
        other => {
            return Err(format!(
                "SMTP_TLS must be starttls, tls or none, not {other}"
            ))
        }
    }
    .map_err(|e| format!("invalid SMTP_HOST: {e}"))?;
    let mut builder = builder;
    if let Ok(port) = std::env::var("SMTP_PORT") {
        let port = port
            .parse::<u16>()
            .map_err(|_| "SMTP_PORT must be a port number".to_string())?;
        builder = builder.port(port);
    }
    if let Ok(username) = std::env::var("SMTP_USERNAME") {
        let password = std::env::var("SMTP_PASSWORD").unwrap_or_default();
        builder = builder.credentials(Credentials::new(username, password));
    }
    Ok(Some(SmtpMailer {
        transport: builder.build(),
        from,
    }))
}

pub fn default_mailer() -> Arc<dyn Mailer> {
    Arc::new(LogMailer)
}

pub fn verify_email_message(link: &str) -> (&'static str, String) {
    (
        "Confirm your QSets email address",
        format!(
            "Confirm your email address for QSets by opening this link:\n\n{link}\n\n\
             The link works once and expires in 24 hours. If you did not ask for this, you can \
             ignore this message.\n"
        ),
    )
}

pub fn reset_password_message(link: &str) -> (&'static str, String) {
    (
        "Reset your QSets password",
        format!(
            "Someone asked to reset the password for the QSets account tied to this address. \
             To choose a new password, open this link:\n\n{link}\n\n\
             The link works once and expires in 1 hour. If you did not ask for this, you can \
             ignore this message; your password has not changed.\n"
        ),
    )
}

/// Sent instead of a verification link when the address already belongs to an account, so a
/// registration form cannot be used to find out which addresses are registered.
pub fn address_in_use_message(reset_url: &str) -> (&'static str, String) {
    (
        "QSets: this address already has an account",
        format!(
            "Someone tried to use this email address to create or change a QSets account, but \
             it already belongs to one. If that was you, you can sign in with this address, or \
             reset your password here:\n\n{reset_url}\n\nIf it was not you, no action is needed.\n"
        ),
    )
}
