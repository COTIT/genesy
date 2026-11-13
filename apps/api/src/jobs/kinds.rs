//! The job kinds this build runs, their handlers and their schedules. A new kind is a const
//! here, a match arm in `dispatch` and, when recurring, a `SCHEDULES` entry.

use std::sync::Arc;

use sqlx::PgPool;

use crate::{
    cleanup,
    error::AppError,
    jobs::{
        repo::Claimed,
        runner::{Handler, Step},
        scheduler::{Every, Schedule},
    },
    modules::{messages::repo as messages, orders, stock::sweep, webhooks::deliver},
    outbound::Outbound,
    storage::Store,
};

/// Marks email sends a dead request left open as `unknown` (every 5 min).
pub const CLOSE_STUCK_SENDS: &str = "close_stuck_sends";
/// Deletes stale rows: tokens, invites, login quotas, old jobs and slots (hourly).
pub const PRUNE: &str = "prune";
/// Finishes file rows a crash left half written or half deleted (hourly).
pub const FILE_SWEEP: &str = "file_sweep";
/// Runs one durable invoice delivery intent.
pub const INVOICE_DELIVERY: &str = "invoice_delivery";
/// Reconcile interrupted invoice transport attempts without replay.
pub const INVOICE_DELIVERY_SWEEP: &str = "invoice_delivery_sweep";
/// Removes ended/orphaned provider conversations every minute.
pub const ASSISTANT_CLEANUP: &str = "assistant_cleanup";
/// Zero stock rows for every product in a new main or store warehouse (`stock::sweep`).
pub const STOCK_SEED: &str = "stock_seed";

/// Turns a workspace's pending webhook events into deliveries (`webhooks::deliver`).
pub const WEBHOOK_FANOUT: &str = "webhook_fanout";

/// POSTs one signed webhook delivery (`webhooks::deliver`).
pub const WEBHOOK_DELIVER: &str = "webhook_deliver";

/// Re-queues fan-outs for events left pending (every minute, `webhooks::deliver::sweep`).
pub const WEBHOOK_SWEEP: &str = "webhook_sweep";

/// Cancels web orders whose payment window passed and gives their stock back (every minute,
/// `orders::checkout::expire`).
pub const ORDER_EXPIRE: &str = "order_expire";

/// Re-fetches payments named by provider webhooks and applies what the provider says
/// (`orders::payments::drain`).
pub const PAYMENT_INBOX: &str = "payment_inbox";

/// Retries payment and refund creates a dead request left behind (same idempotency key) and
/// re-fetches payments left open (every minute, `orders::payments::sweep`).
pub const PAYMENT_SWEEP: &str = "payment_sweep";

/// Mails due scheduled reports (every 5 min, `scheduled_reports::runner`).
pub const SCHEDULED_REPORTS: &str = "scheduled_reports";
/// Revisioned product preparation and approved native writes.
pub const PRODUCT_BULK: &str = "product_bulk";
/// Expire drafts and remove private sources/audit within the import retention policy.
pub const PRODUCT_BULK_PRUNE: &str = "product_bulk_prune";

/// Every kind this build can run; the claim never takes another.
pub const KINDS: &[&str] = &[
    INVOICE_DELIVERY,
    INVOICE_DELIVERY_SWEEP,
    CLOSE_STUCK_SENDS,
    PRUNE,
    FILE_SWEEP,
    ASSISTANT_CLEANUP,
    STOCK_SEED,
    WEBHOOK_FANOUT,
    WEBHOOK_DELIVER,
    WEBHOOK_SWEEP,
    ORDER_EXPIRE,
    PAYMENT_INBOX,
    PAYMENT_SWEEP,
    SCHEDULED_REPORTS,
    PRODUCT_BULK,
    PRODUCT_BULK_PRUNE,
];

/// One sweep chunk outside the rail (tests, load tests): see `stock::repo::seed_chunk`.
pub use crate::modules::stock::repo::seed_chunk;

/// Recurring jobs.
pub static SCHEDULES: [Schedule; 10] = [
    Schedule {
        name: PRODUCT_BULK_PRUNE,
        kind: PRODUCT_BULK_PRUNE,
        every: Every::Minutes(1),
    },
    Schedule {
        name: INVOICE_DELIVERY_SWEEP,
        kind: INVOICE_DELIVERY_SWEEP,
        every: Every::Minutes(5),
    },
    Schedule {
        name: ASSISTANT_CLEANUP,
        kind: ASSISTANT_CLEANUP,
        every: Every::Minutes(1),
    },
    Schedule {
        name: CLOSE_STUCK_SENDS,
        kind: CLOSE_STUCK_SENDS,
        every: Every::Minutes(5),
    },
    Schedule {
        name: PRUNE,
        kind: PRUNE,
        every: Every::Minutes(60),
    },
    Schedule {
        name: FILE_SWEEP,
        kind: FILE_SWEEP,
        every: Every::Minutes(60),
    },
    Schedule {
        name: WEBHOOK_SWEEP,
        kind: WEBHOOK_SWEEP,
        every: Every::Minutes(1),
    },
    Schedule {
        name: ORDER_EXPIRE,
        kind: ORDER_EXPIRE,
        every: Every::Minutes(1),
    },
    Schedule {
        name: PAYMENT_SWEEP,
        kind: PAYMENT_SWEEP,
        every: Every::Minutes(1),
    },
    Schedule {
        name: SCHEDULED_REPORTS,
        kind: SCHEDULED_REPORTS,
        every: Every::Minutes(5),
    },
];

/// What handlers reach: the job pool and, when configured, the file bucket.
#[derive(Clone)]
pub struct Ctx {
    /// Full app state for invoice delivery and scheduled report mail; no serialized job
    /// credentials.
    pub invoice_state: Option<crate::AppState>,
    /// Provider cleanup key; never serialized into jobs or traces.
    pub assistant_api_key: Option<Arc<crate::AssistantSecret>>,
    /// The job pool.
    pub db: PgPool,
    /// The file bucket, when configured.
    pub files: Option<Store>,
    /// Outbound HTTP behind the address guard (webhook deliveries).
    pub http: Outbound,
    /// Opens webhook secrets; None fails deliveries until configured.
    pub credentials: Option<std::sync::Arc<crate::CredentialKey>>,
    /// Web payments (re-fetches, retried creates, refunds); None leaves them waiting.
    pub payments: crate::payments::Payments,
}

/// The runner's handler over `dispatch`.
pub fn handler(ctx: Ctx) -> Handler {
    Arc::new(move |job| {
        let ctx = ctx.clone();
        Box::pin(async move { dispatch(&ctx, job).await })
    })
}

/// Run one job (or one chunk) of a known kind.
pub async fn dispatch(ctx: &Ctx, job: Claimed) -> Result<Step, AppError> {
    match job.kind.as_str() {
        PRODUCT_BULK => return crate::modules::product_bulk_runs::worker::run(ctx, &job).await,
        PRODUCT_BULK_PRUNE => {
            if crate::modules::product_bulk_runs::worker::prune(&ctx.db, ctx.files.as_ref()).await?
                > 0
            {
                return Ok(Step::More);
            }
        }
        INVOICE_DELIVERY => return crate::modules::invoices::delivery::run(ctx, &job).await,
        INVOICE_DELIVERY_SWEEP => return crate::modules::invoices::delivery::sweep(ctx).await,
        ASSISTANT_CLEANUP => return crate::modules::assistant::cleanup::run(ctx).await,
        CLOSE_STUCK_SENDS => {
            messages::close_stuck(&ctx.db).await?;
        }
        PRUNE => {
            let pruned = cleanup::prune(&ctx.db).await?;
            tracing::info!(?pruned, "prune done");
        }
        FILE_SWEEP => {
            if let Some(bucket) = &ctx.files {
                let swept = cleanup::sweep_files(&ctx.db, bucket)
                    .await
                    .map_err(AppError::Internal)?;
                tracing::info!(files = swept, "file sweep done");
            }
        }
        STOCK_SEED => return sweep::run(&ctx.db, &job).await,
        WEBHOOK_FANOUT => return deliver::fanout(&ctx.db, &job).await,
        WEBHOOK_DELIVER => return deliver::deliver(ctx, &job).await,
        WEBHOOK_SWEEP => return deliver::sweep(&ctx.db).await,
        ORDER_EXPIRE => return orders::checkout::expire(&ctx.db).await,
        PAYMENT_INBOX => return orders::payments::drain(ctx, &job).await,
        PAYMENT_SWEEP => return orders::payments::sweep(ctx).await,
        SCHEDULED_REPORTS => match &ctx.invoice_state {
            Some(state) => return crate::modules::scheduled_reports::runner::run(state).await,
            None => tracing::warn!("scheduled reports wait: no mailer in the job context"),
        },
        // The claim only takes `KINDS`, so this means a kind was listed without an arm.
        other => {
            return Err(AppError::Internal(format!(
                "no handler for job kind {other}"
            )));
        }
    }
    Ok(Step::Done)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::*;

    fn claimed(kind: &str) -> Claimed {
        Claimed {
            id: Uuid::now_v7(),
            kind: kind.into(),
            workspace_id: None,
            payload: json!({}),
            attempts: 1,
            max_attempts: 5,
            created_by: None,
            token: Uuid::now_v7(),
        }
    }

    fn ctx(db: &PgPool) -> Ctx {
        Ctx {
            invoice_state: None,
            assistant_api_key: None,
            db: db.clone(),
            files: None,
            http: Outbound::new(&[]).unwrap(),
            credentials: None,
            payments: Default::default(),
        }
    }

    async fn count(db: &PgPool, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(db).await.unwrap()
    }

    #[test]
    fn every_schedule_runs_a_kind_this_build_knows() {
        let names: HashSet<_> = SCHEDULES.iter().map(|s| s.name).collect();
        assert_eq!(names.len(), SCHEDULES.len(), "schedule names are unique");
        assert!(SCHEDULES.iter().all(|s| KINDS.contains(&s.kind)));
    }

    #[test]
    fn every_schedule_is_a_valid_time() {
        for s in &SCHEDULES {
            match s.every {
                Every::Minutes(n) => assert!((1..=1440).contains(&n), "{}", s.name),
                Every::DailyAt { hour, minute } => {
                    assert!(hour < 24 && minute < 60, "{}", s.name);
                }
            }
        }
    }

    #[sqlx::test]
    async fn prune_kind_also_prunes_jobs_and_slots(db: PgPool) {
        sqlx::query(
            "INSERT INTO jobs (kind, status, finished_at) VALUES ('x', 'succeeded', now() - interval '8 days')",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO scheduled_runs (name, slot, created_at) VALUES ('x', now(), now() - interval '31 days')",
        )
        .execute(&db)
        .await
        .unwrap();

        assert_eq!(
            dispatch(&ctx(&db), claimed(PRUNE)).await.unwrap(),
            Step::Done
        );

        assert_eq!(count(&db, "SELECT count(*) FROM jobs").await, 0);
        assert_eq!(count(&db, "SELECT count(*) FROM scheduled_runs").await, 0);
    }

    #[sqlx::test]
    async fn close_stuck_sends_kind_closes_open_sends(db: PgPool) {
        sqlx::query(
            "WITH u AS (INSERT INTO users (email, first_name, last_name, password_hash)
                        VALUES ('o@x.io', 'O', 'W', 'h') RETURNING id),
                  w AS (INSERT INTO workspaces (name, created_by_user_id)
                        SELECT 'acme', id FROM u RETURNING id)
             INSERT INTO email_send_logs (id, workspace_id, provider, from_email, subject, updated_at)
             SELECT uuidv7(), id, 'smtp', 'a@b.co', 's', now() - interval '11 minutes' FROM w",
        )
        .execute(&db)
        .await
        .unwrap();

        assert_eq!(
            dispatch(&ctx(&db), claimed(CLOSE_STUCK_SENDS))
                .await
                .unwrap(),
            Step::Done
        );

        let open = count(
            &db,
            "SELECT count(*) FROM email_send_logs WHERE status IN ('pending', 'sending')",
        )
        .await;
        assert_eq!(open, 0);
    }

    #[sqlx::test]
    async fn file_sweep_without_a_bucket_does_nothing(db: PgPool) {
        assert_eq!(
            dispatch(&ctx(&db), claimed(FILE_SWEEP)).await.unwrap(),
            Step::Done
        );
    }
}
