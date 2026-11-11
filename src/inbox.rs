#![doc = include_str!("../docs/inbox.md")]

use crate::{
    ack::{self, Code, Issue, Mode, Reply, Severity},
    wire::{self, Message},
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::{fmt, time::Duration};

/// Initial schema; apply once through the host's migration mechanism, never on each receipt.
pub const SCHEMA: &str = include_str!("../migrations/0001_inbox.sql");
pub const WORK_SCHEMA: &str = include_str!("../migrations/0002_work.sql");
mod work;
pub use work::{Claim, ClaimOptions, Lease, Outcome, State};
const MAX_PAYLOAD: usize = 8 * 1024 * 1024;
const MAX_ACK: usize = 64 * 1024;

#[derive(Debug)]
pub enum Error {
    Database(sqlx::Error),
    Ack(ack::Error),
    Wire(wire::Error),
    Configuration,
    Durability,
    Integrity,
    LeaseLost,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HL7 inbox {self:?}")
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            Self::Ack(e) => Some(e),
            Self::Wire(e) => Some(e),
            _ => None,
        }
    }
}
impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}
impl From<ack::Error> for Error {
    fn from(e: ack::Error) -> Self {
        Self::Ack(e)
    }
}
impl From<wire::Error> for Error {
    fn from(e: wire::Error) -> Self {
        Self::Wire(e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    New,
    Duplicate,
    Conflict,
}
impl Disposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Duplicate => "duplicate",
            Self::Conflict => "conflict",
        }
    }
}
pub struct AckMetadata<'a> {
    pub control_id: &'a [u8],
    pub timestamp: &'a [u8],
}
pub struct Receipt {
    pub message_id: i64,
    pub delivery_id: i64,
    pub disposition: Disposition,
    pub plan: [u8; 32],
    pub acknowledgement: Vec<u8>,
}
impl fmt::Debug for Receipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Receipt")
            .field("message_id", &self.message_id)
            .field("delivery_id", &self.delivery_id)
            .field("disposition", &self.disposition)
            .finish_non_exhaustive()
    }
}

/// A caller-owned pool; its connection/TLS, admission and acquisition limits remain host policy.
#[derive(Clone)]
pub struct Inbox {
    pool: PgPool,
    max_payload: usize,
    statement_ms: u32,
}
impl Inbox {
    pub fn new(
        pool: PgPool,
        max_payload: usize,
        statement_timeout: Duration,
    ) -> Result<Self, Error> {
        let statement_ms =
            u32::try_from(statement_timeout.as_millis()).map_err(|_| Error::Configuration)?;
        if max_payload == 0
            || max_payload > MAX_PAYLOAD
            || statement_ms == 0
            || statement_ms > i32::MAX as u32
        {
            return Err(Error::Configuration);
        }
        Ok(Self {
            pool,
            max_payload,
            statement_ms,
        })
    }

    /// Return an acknowledgement only after committing original, outcome, plan and ACK bytes.
    pub async fn receive(
        &self,
        scope: &str,
        channel: &str,
        message: &Message,
        plan: &[u8; 32],
        response: AckMetadata<'_>,
    ) -> Result<Receipt, Error> {
        if scope.is_empty()
            || scope.len() > 512
            || scope.chars().any(char::is_control)
            || channel.is_empty()
            || channel.len() > 1024
            || channel.chars().any(char::is_control)
            || message.original().len() > self.max_payload
        {
            return Err(Error::Configuration);
        }
        ack::validate_request(message, Mode::EnhancedAcceptOnly)?;
        let control = message.value(&[1, 10]).decode()?;
        if control.len() > 256 {
            return Err(Error::Configuration);
        }
        let processing = message.value(&[1, 11, 1, 1]).decode()?;
        let positive = ack::build(
            message,
            Reply {
                code: Code::CA,
                control_id: response.control_id,
                timestamp: response.timestamp,
                issue: None,
            },
            MAX_ACK,
        )?;
        let negative = ack::build(
            message,
            Reply {
                code: Code::CE,
                control_id: response.control_id,
                timestamp: response.timestamp,
                issue: Some(Issue {
                    code: 207,
                    severity: Severity::Error,
                    diagnostic: b"Control ID reused with different payload",
                }),
            },
            MAX_ACK,
        )?;
        let digest = Sha256::digest(message.original());
        let (mut tx, commit_mode) = self.transaction().await?;
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO hl7v2.inbox
            (scope, processing_id, control_id, plan, payload, sha256, acknowledgement)
            VALUES ($1,$2,$3,$4,$5,$6,$7)
            ON CONFLICT (scope, processing_id, control_id) DO NOTHING RETURNING id",
        )
        .bind(scope)
        .bind(processing.as_ref())
        .bind(control.as_ref())
        .bind(plan.as_slice())
        .bind(message.original())
        .bind(digest.as_slice())
        .bind(&positive)
        .fetch_optional(&mut *tx)
        .await?;
        let (message_id, disposition, effective_plan, acknowledgement) = if let Some(id) = inserted
        {
            (id, Disposition::New, *plan, positive)
        } else {
            // A fresh READ COMMITTED statement sees the winner after ON CONFLICT has waited.
            let original = sqlx::query(
                "SELECT id, payload, sha256, plan, acknowledgement
                FROM hl7v2.inbox WHERE scope=$1 AND processing_id=$2 AND control_id=$3",
            )
            .bind(scope)
            .bind(processing.as_ref())
            .bind(control.as_ref())
            .fetch_one(&mut *tx)
            .await?;
            let effective_plan = original
                .try_get::<Vec<u8>, _>("plan")?
                .try_into()
                .map_err(|_| Error::Integrity)?;
            let equal = original.try_get::<&[u8], _>("payload")? == message.original();
            if equal && original.try_get::<&[u8], _>("sha256")? != digest.as_slice() {
                return Err(Error::Integrity);
            }
            let acknowledgement = if equal {
                original.try_get("acknowledgement")?
            } else {
                negative
            };
            (
                original.try_get("id")?,
                if equal {
                    Disposition::Duplicate
                } else {
                    Disposition::Conflict
                },
                effective_plan,
                acknowledgement,
            )
        };
        let delivery_id = sqlx::query_scalar("INSERT INTO hl7v2.deliveries
            (message_id, channel, requested_plan, payload, sha256, disposition, acknowledgement, commit_mode)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id")
            .bind(message_id).bind(channel).bind(plan.as_slice()).bind(message.original())
            .bind(digest.as_slice()).bind(disposition.as_str()).bind(&acknowledgement).bind(commit_mode)
            .fetch_one(&mut *tx).await?;
        tx.commit().await?;
        Ok(Receipt {
            message_id,
            delivery_id,
            disposition,
            plan: effective_plan,
            acknowledgement,
        })
    }

    async fn transaction(&self) -> Result<(Transaction<'static, Postgres>, String), Error> {
        let mut tx = self
            .pool
            .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
            .await?;
        let settings = sqlx::query("SELECT set_config('statement_timeout', $1, true),
            current_setting('fsync') AS fsync, current_setting('full_page_writes') AS full_page_writes,
            current_setting('synchronous_commit') AS commit_mode")
            .bind(format!("{}ms", self.statement_ms)).fetch_one(&mut *tx).await?;
        let commit_mode: String = settings.try_get("commit_mode")?;
        if settings.try_get::<&str, _>("fsync")? != "on"
            || settings.try_get::<&str, _>("full_page_writes")? != "on"
            || !matches!(
                commit_mode.as_str(),
                "on" | "local" | "remote_write" | "remote_apply"
            )
        {
            return Err(Error::Durability);
        }
        Ok((tx, commit_mode))
    }
}
