#![doc = include_str!("../../docs/work.md")]
use super::{Error, Inbox};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use std::{fmt, sync::Arc, time::Duration};

#[derive(Debug, Clone, Copy)]
pub struct ClaimOptions {
    pub lease_for: Duration,
    pub max_attempts: u16,
}
impl Default for ClaimOptions {
    fn default() -> Self {
        Self {
            lease_for: Duration::from_secs(60),
            max_attempts: 5,
        }
    }
}
#[derive(Debug)]
pub enum Claim {
    Empty,
    Work(Lease),
    Quarantined { message_id: i64 },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Completed,
    RetryScheduled,
    Quarantined,
}
impl State {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::RetryScheduled => "retry_scheduled",
            Self::Quarantined => "quarantined",
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub enum Outcome {
    Completed,
    RetryAfter(Duration),
    Quarantine,
}

/// A private generation token bound to its originating pool; clones share source bytes.
#[derive(Clone)]
pub struct Lease {
    inbox: Inbox,
    message_id: i64,
    generation: i64,
    plan: [u8; 32],
    payload: Arc<[u8]>,
}
impl fmt::Debug for Lease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lease")
            .field("message_id", &self.message_id)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}
fn milliseconds(duration: Duration, maximum: u64) -> Result<i64, Error> {
    let ms = u64::try_from(duration.as_millis()).map_err(|_| Error::Configuration)?;
    if ms == 0 || ms > maximum {
        return Err(Error::Configuration);
    }
    Ok(ms as i64)
}

impl Inbox {
    /// Claim at most one due record for a known, exact plan. No background task is started.
    pub async fn claim(
        &self,
        scope: &str,
        worker: &str,
        plan: &[u8; 32],
        options: ClaimOptions,
    ) -> Result<Claim, Error> {
        let ms = milliseconds(options.lease_for, 3_600_000)?;
        if scope.is_empty()
            || scope.len() > 512
            || scope.chars().any(char::is_control)
            || worker.is_empty()
            || worker.len() > 256
            || worker.chars().any(char::is_control)
            || !(1..=1000).contains(&options.max_attempts)
        {
            return Err(Error::Configuration);
        }
        let (mut tx, commit_mode) = self.transaction().await?;
        let row = sqlx::query("SELECT id, generation, attempt_limit, state, payload, sha256, plan
            FROM hl7v2.inbox WHERE scope=$1 AND plan=$2
            AND state IN ('ready','processing','retry_scheduled') AND available_at<=clock_timestamp()
            ORDER BY available_at,id LIMIT 1 FOR UPDATE SKIP LOCKED")
            .bind(scope).bind(plan.as_slice()).fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(Claim::Empty);
        };
        let id: i64 = row.try_get("id")?;
        let generation: i64 = row.try_get("generation")?;
        let limit = row
            .try_get::<Option<i32>, _>("attempt_limit")?
            .unwrap_or(i32::from(options.max_attempts));
        if row.try_get::<&str, _>("state")? == "processing" {
            let ended = sqlx::query(
                "UPDATE hl7v2.attempts SET ended_at=clock_timestamp(), outcome='expired'
                WHERE message_id=$1 AND generation=$2 AND outcome IS NULL",
            )
            .bind(id)
            .bind(generation)
            .execute(&mut *tx)
            .await?;
            if ended.rows_affected() != 1 {
                return Err(Error::Integrity);
            }
        }
        if generation >= i64::from(limit) {
            sqlx::query("UPDATE hl7v2.inbox SET state='quarantined', quarantine_reason='attempt_limit' WHERE id=$1")
                .bind(id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(Claim::Quarantined { message_id: id });
        }
        let payload: &[u8] = row.try_get("payload")?;
        if payload.len() > self.max_payload {
            return Err(Error::Configuration);
        }
        if row.try_get::<&[u8], _>("sha256")? != Sha256::digest(payload).as_slice() {
            return Err(Error::Integrity);
        }
        let plan = row
            .try_get::<&[u8], _>("plan")?
            .try_into()
            .map_err(|_| Error::Integrity)?;
        let payload = Arc::from(payload);
        drop(row);
        let generation = sqlx::query_scalar("UPDATE hl7v2.inbox SET state='processing', generation=generation+1,
            attempt_limit=$2, available_at=clock_timestamp()+$3*interval '1 millisecond' WHERE id=$1 RETURNING generation")
            .bind(id).bind(limit).bind(ms).fetch_one(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO hl7v2.attempts(message_id,generation,worker,plan,lease_until,commit_mode)
            SELECT id,generation,$2,plan,available_at,$3 FROM hl7v2.inbox WHERE id=$1",
        )
        .bind(id)
        .bind(worker)
        .bind(commit_mode)
        .execute(&mut *tx)
        .await?;
        let lease = Lease {
            inbox: self.clone(),
            message_id: id,
            generation,
            plan,
            payload,
        };
        lease.active(&mut tx).await?;
        tx.commit().await?;
        Ok(Claim::Work(lease))
    }
}

impl Lease {
    pub fn message_id(&self) -> i64 {
        self.message_id
    }
    pub fn generation(&self) -> i64 {
        self.generation
    }
    pub fn plan(&self) -> &[u8; 32] {
        &self.plan
    }
    pub fn payload(&self) -> &Arc<[u8]> {
        &self.payload
    }

    async fn active(&self, tx: &mut Transaction<'_, Postgres>) -> Result<i32, Error> {
        sqlx::query_scalar(
            "SELECT attempt_limit FROM hl7v2.inbox WHERE id=$1
            AND generation=$2 AND state='processing' AND available_at>clock_timestamp()",
        )
        .bind(self.message_id)
        .bind(self.generation)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(Error::LeaseLost)
    }
    async fn lock(&self) -> Result<(Transaction<'static, Postgres>, i32), Error> {
        let (mut tx, _) = self.inbox.transaction().await?;
        let row: Option<i64> =
            sqlx::query_scalar("SELECT id FROM hl7v2.inbox WHERE id=$1 FOR UPDATE")
                .bind(self.message_id)
                .fetch_optional(&mut *tx)
                .await?;
        if row.is_none() {
            return Err(Error::LeaseLost);
        }
        // Check wall-clock expiry after waiting for the row lock, not at transaction start.
        let limit = self.active(&mut tx).await?;
        Ok((tx, limit))
    }
    pub async fn renew(&self, lease_for: Duration) -> Result<(), Error> {
        let ms = milliseconds(lease_for, 3_600_000)?;
        let (mut tx, _) = self.lock().await?;
        let changed = sqlx::query("UPDATE hl7v2.inbox SET available_at=greatest(available_at,clock_timestamp()+$3*interval '1 millisecond')
            WHERE id=$1 AND generation=$2 AND state='processing' AND available_at>clock_timestamp()")
            .bind(self.message_id).bind(self.generation).bind(ms).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::LeaseLost);
        }
        let changed = sqlx::query("UPDATE hl7v2.attempts SET lease_until=(SELECT available_at FROM hl7v2.inbox WHERE id=$1)
            WHERE message_id=$1 AND generation=$2 AND outcome IS NULL")
            .bind(self.message_id).bind(self.generation).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::Integrity);
        }
        self.active(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Finalize inbox metadata only. External clinical writes require their own atomic fenced sink.
    pub async fn finish(&self, outcome: Outcome, report: &[u8]) -> Result<State, Error> {
        if report.len() > 65536 {
            return Err(Error::Configuration);
        }
        let delay = match outcome {
            Outcome::RetryAfter(delay) => milliseconds(delay, 604_800_000)?,
            _ => 0,
        };
        let (mut tx, limit) = self.lock().await?;
        let exhausted =
            matches!(outcome, Outcome::RetryAfter(_)) && self.generation >= i64::from(limit);
        let state = match outcome {
            Outcome::Completed => State::Completed,
            Outcome::RetryAfter(_) if !exhausted => State::RetryScheduled,
            _ => State::Quarantined,
        };
        let reason = if exhausted {
            Some("attempt_limit")
        } else if state == State::Quarantined {
            Some("requested")
        } else {
            None
        };
        let changed = sqlx::query(
            "UPDATE hl7v2.attempts SET ended_at=clock_timestamp(), outcome=$3, report=$4
            WHERE message_id=$1 AND generation=$2 AND outcome IS NULL",
        )
        .bind(self.message_id)
        .bind(self.generation)
        .bind(if exhausted {
            "retry_limit"
        } else {
            state.as_str()
        })
        .bind(report)
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(Error::Integrity);
        }
        let changed = sqlx::query("UPDATE hl7v2.inbox SET state=$3, quarantine_reason=$4,
            available_at=clock_timestamp()+$5*interval '1 millisecond'
            WHERE id=$1 AND generation=$2 AND state='processing' AND available_at>clock_timestamp()")
            .bind(self.message_id).bind(self.generation).bind(state.as_str()).bind(reason).bind(delay)
            .execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(Error::LeaseLost);
        }
        tx.commit().await?;
        Ok(state)
    }
}
