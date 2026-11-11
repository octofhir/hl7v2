#![doc = include_str!("../../docs/replay.md")]
use super::{Error, Inbox};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};

/// An authorized host command for another bounded run with the original processing plan.
#[derive(Clone, Copy)]
pub struct ReplayRequest<'a> {
    pub request_id: &'a str,
    pub message_id: i64,
    pub expected_generation: i64,
    pub plan: &'a [u8; 32],
    pub additional_attempts: u16,
    pub operator: &'a str,
    pub reason: &'a str,
}
fn text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

impl Inbox {
    /// Schedule quarantined work atomically; exact retries return the same durable audit ID.
    pub async fn replay(&self, scope: &str, request: ReplayRequest<'_>) -> Result<i64, Error> {
        if !text(scope, 512)
            || !text(request.request_id, 128)
            || !text(request.operator, 256)
            || !text(request.reason, 2048)
            || request.message_id <= 0
            || request.expected_generation <= 0
            || !(1..=1000).contains(&request.additional_attempts)
        {
            return Err(Error::Configuration);
        }
        let ceiling = request
            .expected_generation
            .checked_add(i64::from(request.additional_attempts))
            .ok_or(Error::Configuration)?;
        let (mut tx, commit_mode) = self.transaction().await?;
        let row = sqlx::query(
            "SELECT state,generation,plan,payload,sha256,attempt_limit,quarantine_reason
            FROM hl7v2.inbox WHERE id=$1 AND scope=$2 FOR UPDATE",
        )
        .bind(request.message_id)
        .bind(scope)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::ReplayRejected)?;
        if let Some(id) = existing(&mut tx, scope, &request).await? {
            tx.commit().await?;
            return Ok(id);
        }
        if row.try_get::<&str, _>("state")? != "quarantined"
            || row.try_get::<i64, _>("generation")? != request.expected_generation
            || row.try_get::<&[u8], _>("plan")? != request.plan
        {
            return Err(Error::ReplayRejected);
        }
        let payload: &[u8] = row.try_get("payload")?;
        if payload.len() > self.max_payload {
            return Err(Error::Configuration);
        }
        if row.try_get::<&[u8], _>("sha256")? != Sha256::digest(payload).as_slice() {
            return Err(Error::Integrity);
        }
        let inserted: Option<i64> = sqlx::query_scalar("INSERT INTO hl7v2.replays
            (scope,request_id,message_id,generation,plan,additional_attempts,previous_limit,previous_reason,operator,reason,commit_mode)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT (scope,request_id) DO NOTHING RETURNING id")
            .bind(scope).bind(request.request_id).bind(request.message_id).bind(request.expected_generation)
            .bind(request.plan.as_slice()).bind(i32::from(request.additional_attempts))
            .bind(row.try_get::<i64, _>("attempt_limit")?).bind(row.try_get::<&str, _>("quarantine_reason")?)
            .bind(request.operator).bind(request.reason).bind(commit_mode).fetch_optional(&mut *tx).await?;
        let id = if let Some(id) = inserted {
            sqlx::query(
                "UPDATE hl7v2.inbox SET state='ready', available_at=clock_timestamp(),
                attempt_limit=$2, quarantine_reason=NULL, replay_id=$3 WHERE id=$1",
            )
            .bind(request.message_id)
            .bind(ceiling)
            .bind(id)
            .execute(&mut *tx)
            .await?;
            id
        } else {
            // Another message may have concurrently used this scope's command ID.
            existing(&mut tx, scope, &request)
                .await?
                .ok_or(Error::Integrity)?
        };
        tx.commit().await?;
        Ok(id)
    }
}

async fn existing(
    tx: &mut Transaction<'_, Postgres>,
    scope: &str,
    request: &ReplayRequest<'_>,
) -> Result<Option<i64>, Error> {
    let row = sqlx::query(
        "SELECT id,message_id,generation,plan,additional_attempts,operator,reason
        FROM hl7v2.replays WHERE scope=$1 AND request_id=$2",
    )
    .bind(scope)
    .bind(request.request_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.try_get::<i64, _>("message_id")? != request.message_id
        || row.try_get::<i64, _>("generation")? != request.expected_generation
        || row.try_get::<&[u8], _>("plan")? != request.plan
        || row.try_get::<i32, _>("additional_attempts")? != i32::from(request.additional_attempts)
        || row.try_get::<&str, _>("operator")? != request.operator
        || row.try_get::<&str, _>("reason")? != request.reason
    {
        return Err(Error::ReplayConflict);
    }
    Ok(Some(row.try_get("id")?))
}
