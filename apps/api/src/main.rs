//! Boot: env -> config -> DB pool -> migrations -> serve, graceful stop on SIGINT/SIGTERM.

use std::{net::SocketAddr, time::Duration};

use genesy_360_api::{
    AppState, RateLimit, Slots, WorkspaceSlots, build_app,
    config::Config,
    db, healthcheck, jobs,
    mail::{self, Mailer},
    outbound, payments, pdf, storage, warm_up,
};
use sqlx::postgres::PgConnectOptions;
use tracing_subscriber::EnvFilter;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("parse-product-source") {
        std::process::exit(genesy_360_api::product_bulk_runs::parser::child_main());
    }
    serve();
}

#[tokio::main]
async fn serve() {
    // `genesy-360-api healthcheck`: compose healthcheck for the shell-less image. Needs only
    // BIND_ADDR's port, not the full config (secrets, DB), and exits right away.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        let port = std::env::var("BIND_ADDR")
            .ok()
            .and_then(|addr| addr.parse::<SocketAddr>().ok())
            .map_or(8080, |addr| addr.port());
        std::process::exit(if healthcheck::probe(port).await { 0 } else { 1 });
    }

    // .env optional: dev convenience, prod injects real env.
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // `genesy-360-api migrate`: the deploy's migration step (one-off container), then exit.
    // Reads only DATABASE_URL(_FILE), so CI can migrate its template DB without app config.
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        let result = match Config::database_url_from_env()
            .and_then(|url| url.parse::<PgConnectOptions>().map_err(|e| e.to_string()))
        {
            Ok(base) => db::migrate(&base).await.map_err(|e| e.to_string()),
            Err(err) => Err(err),
        };
        match result {
            Ok(migrated) => println!("{migrated}"),
            Err(err) => {
                eprintln!("migrate: {err}");
                std::process::exit(1);
            }
        }
        return;
    }

    let mut config = Config::from_env().expect("config");
    let base: PgConnectOptions = config.database_url.parse().expect("DATABASE_URL");
    // Migrations are embedded in the binary and run on their own connection, before the pool.
    // The VPS composes skip this: their deploy runs `migrate` first.
    if !config.skip_boot_migrations {
        db::migrate(&base).await.expect("run migrations");
    }
    let health_db = db::health(base.clone())
        .await
        .expect("connect postgres (health)");
    let background_db = db::background(base.clone())
        .await
        .expect("connect postgres (background)");
    let jobs_db = db::jobs(base.clone(), config.db_job_connections)
        .await
        .expect("connect postgres (jobs)");
    let db = db::connect(base, config.db_max_connections)
        .await
        .expect("connect postgres");
    warm_up().await;

    // Operator-configured endpoints may be internal (the sidecar, a local MinIO): trusted.
    // Graph hosts count only when overridden (a local fake); Microsoft's are public anyway.
    let graph_fakes = [&config.graph_login_url, &config.graph_api_url]
        .into_iter()
        .filter(|u| !u.starts_with(mail::graph::LOGIN_URL) && !u.starts_with(mail::graph::API_URL))
        .map(String::as_str);
    let trusted: Vec<String> = [
        config.gotenberg_url.as_deref(),
        config.storage.as_ref().map(|s| s.endpoint.as_str()),
    ]
    .into_iter()
    .flatten()
    .chain(graph_fakes)
    .filter_map(outbound::host_of)
    .collect();
    let http = outbound::Outbound::new(&trusted).expect("outbound http client");
    let pdf = config
        .gotenberg_url
        .as_deref()
        .map(|url| pdf::Renderer::new(url, http.clone()));
    let files = config
        .storage
        .take()
        .map(|s| storage::Store::s3(s, http.clone()).expect("file storage"));
    let job_files = files.clone();
    // Webhook targets are customer URLs: no operator host is trusted for them.
    let job_http = outbound::Outbound::new(&[]).expect("outbound http client");
    let credentials = config.credentials_key.map(std::sync::Arc::new);
    // Checked by Config: fake is refused in a release build.
    let payments = payments::select(
        config.payments_provider.as_deref(),
        !cfg!(debug_assertions),
        config.stripe.clone(),
    )
    .expect("payments provider");
    let mut payments =
        payments::Payments::from(payments).with_globadyme_environment(config.globadyme_environment);
    if let Some(settings) = config.stripe.clone() {
        payments = payments.retain(payments::Provider::Stripe(
            payments::stripe::Stripe::new(settings).expect("retained Stripe provider"),
        ));
    }
    if payments.is_none() {
        tracing::warn!("PAYMENTS_PROVIDER not set: web payments answer 503");
    }
    let job_credentials = credentials.clone();
    if files.is_none() || pdf.is_none() {
        tracing::warn!(
            files = files.is_some(),
            pdf = pdf.is_some(),
            "file storage or PDF rendering not configured: those routes answer 503"
        );
    }
    if credentials.is_none() {
        tracing::warn!("CREDENTIALS_KEY not set: integration credentials can't be stored");
    }
    let state = AppState {
        db,
        health_db,
        token_ttl: chrono::Duration::days(config.token_ttl_days),
        mailer: Mailer::smtp(&config.smtp_url, &config.mail_from)
            .expect("mailer config")
            .with_allowed_domains(&config.mail_allowed_domains)
            .with_trusted_smtp_hosts(&config.mail_trusted_smtp_hosts)
            .with_graph(mail::graph::Client::new(
                &config.graph_login_url,
                &config.graph_api_url,
                http.clone(),
            )),
        frontend_url: config.frontend_url.as_str().into(),
        api_public_url: config.api_public_url.as_str().into(),
        shop_public_url: config.shop_public_url.as_deref().map(Into::into),
        rate_limit: RateLimit {
            burst: config.rate_limit_burst,
            replenish: Duration::from_secs(config.rate_limit_replenish_secs),
            trust_forwarded: config.trust_proxy_headers,
        },
        invites_per_hour: config.invites_per_hour,
        private_rate_limit: RateLimit {
            burst: config.private_rate_limit_burst,
            replenish: Duration::from_millis(config.private_rate_limit_replenish_ms),
            trust_forwarded: config.trust_proxy_headers,
        },
        private_ip_rate_limit: RateLimit {
            burst: config.private_ip_rate_limit_burst,
            replenish: Duration::from_millis(config.private_ip_rate_limit_replenish_ms),
            trust_forwarded: config.trust_proxy_headers,
        },
        preview_rate_limit: RateLimit {
            burst: config.preview_rate_limit_burst,
            replenish: Duration::from_millis(config.preview_rate_limit_replenish_ms),
            trust_forwarded: config.trust_proxy_headers,
        },
        slots: Slots::new(WorkspaceSlots {
            per_workspace: config.workspace_max_inflight,
            per_user: config.user_max_inflight,
            // Same as the pool's acquire timeout: a queued request gives up when a pooled one would.
            wait: db::ACQUIRE_TIMEOUT,
        }),
        request_timeout: Duration::from_secs(config.request_timeout_secs),
        body_deadline: Duration::from_secs(config.request_body_deadline_secs),
        credentials,
        assistant_bridge_secret: config.assistant_bridge_secret.map(std::sync::Arc::new),
        product_bulk_enabled: config.product_bulk_enabled,
        anthropic_api_key: config.anthropic_api_key.map(std::sync::Arc::new),
        files,
        pdf,
        payments: payments.clone(),
        checkout_hold_max: config.checkout_hold_max,
        signup_enabled: config.signup_enabled,
    };
    // `genesy-360-api seed-farm-demo`: the RUAHA demo workspace, then exit. CLI only: no route
    // ever seeds. Runs before the listener binds, so it can run beside a live server.
    if std::env::args().nth(1).as_deref() == Some("seed-farm-demo") {
        use genesy_360_api::farm_demo::{self, STAFF, WORKSPACE};
        match farm_demo::run(state).await {
            Ok(r) if r.created => {
                // Printed once, to stdout only: the passwords exist nowhere else.
                let staff = r.staff_password.unwrap_or_default();
                let farmer = r.farmer_password.unwrap_or_default();
                println!("seeded workspace {WORKSPACE} ({})", r.workspace_id);
                println!("staff, password {staff}:");
                println!("  amina.admin@ruaha-demo.test  admin");
                for (email, _, _, role, village) in STAFF {
                    println!("  {email}  {role} {}", village.unwrap_or(""));
                }
                println!(
                    "farmers (POST /v1/farm/login), password {farmer}: 0712000001, 0712000003, 0712000004"
                );
            }
            Ok(r) => println!(
                "{WORKSPACE} already seeded ({}): nothing to do",
                r.workspace_id
            ),
            Err(err) => {
                eprintln!("seed-farm-demo: {err}");
                std::process::exit(1);
            }
        }
        return;
    }
    // `genesy-360-api seed-demo`: every part of the staging demo, then exit. Refuses unless
    // GENESY_ALLOW_DEMO_SEED is set (staging compose only). Secrets go to stdout once.
    if std::env::args().nth(1).as_deref() == Some("seed-demo") {
        // `--images` (staging's seed service): AFM's photos into the file store too.
        let photos = std::env::args()
            .any(|a| a == "--images")
            .then(|| genesy_360_api::demo::photos::Photos::afm().expect("outbound http client"));
        match genesy_360_api::demo::run_with_photos(state, config.allow_demo_seed, photos).await {
            Ok(s) if s.created => {
                println!(
                    "{:<16} | {:<14} | {:<32} | {:<40} | path",
                    "app", "role", "login", "secret"
                );
                for c in &s.credentials {
                    println!(
                        "{:<16} | {:<14} | {:<32} | {:<40} | {}",
                        c.app, c.role, c.login, c.secret, c.path
                    );
                }
                println!(
                    "storefront app key (shop container): {}",
                    s.storefront_key.unwrap_or_default()
                );
                println!(
                    "passport (QR target): {}",
                    s.passport_path.unwrap_or_default()
                );
                for (label, path) in &s.samples {
                    println!("{label}: {path}");
                }
                println!(
                    "note: web payments went through the seed's in-process Fake provider; \
                     their payment rows say `fake`."
                );
                if let Some((missing, total)) = s.photos.filter(|(m, _)| *m > 0) {
                    eprintln!(
                        "seed-demo: {missing} of {total} product photos unavailable (AFM's image \
                         CDN unreachable or no file storage); those products have no photo"
                    );
                }
                println!("seeded in {:.1} s", s.elapsed.as_secs_f64());
            }
            Ok(_) => println!("staging demo already seeded: nothing to do"),
            Err(err) => {
                eprintln!("seed-demo: {err}");
                std::process::exit(1);
            }
        }
        return;
    }
    // `genesy-360-api create-owner --email E --workspace NAME [--first-name F] [--last-name L]`:
    // the first owner of a production install (signup is off there). Refuses an existing email.
    if std::env::args().nth(1).as_deref() == Some("create-owner") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        let arg = |name: &str| {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let (Some(email), Some(workspace)) = (arg("--email"), arg("--workspace")) else {
            eprintln!(
                "usage: create-owner --email <email> --workspace <name> [--first-name F] [--last-name L]"
            );
            std::process::exit(2);
        };
        let first = arg("--first-name").unwrap_or_else(|| "Owner".into());
        let last = arg("--last-name").unwrap_or_else(|| "Admin".into());
        match genesy_360_api::bootstrap::create_owner(&state, &email, &workspace, &first, &last)
            .await
        {
            Ok(owner) => {
                // Printed once, to stdout only: the password exists nowhere else.
                println!("created workspace {workspace} ({})", owner.workspace_id);
                println!("owner {email}, one-time password: {}", owner.password);
                println!("sign in at /login and change it now");
            }
            Err(err) => {
                eprintln!("create-owner: {err}");
                std::process::exit(1);
            }
        }
        return;
    }
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .expect("bind listener");
    tracing::info!(addr = %config.bind_addr, "listening");

    // The job rail starts last: a boot step that panics above must not leave claimed jobs
    // behind in a process that dies.
    let (stop_jobs, jobs_stopped) = tokio::sync::watch::channel(false);
    let runner = jobs::runner::Runner {
        db: jobs_db.clone(),
        control: background_db.clone(),
        kinds: jobs::kinds::KINDS.to_vec(),
        workers: usize::try_from(config.db_job_connections).unwrap_or(1),
        cap: i64::from(config.job_max_running_per_workspace),
        lease: jobs::LEASE,
        idle: jobs::IDLE,
        max_run: jobs::MAX_RUN,
        handler: jobs::kinds::handler(jobs::kinds::Ctx {
            invoice_state: Some(AppState {
                db: jobs_db.clone(),
                ..state.clone()
            }),
            db: jobs_db,
            files: job_files,
            http: job_http,
            credentials: job_credentials,
            assistant_api_key: config.elevenlabs_api_key.map(std::sync::Arc::new),
            payments: payments.clone(),
        }),
    }
    .spawn(jobs_stopped.clone());
    let scheduler =
        jobs::scheduler::spawn(background_db, &jobs::kinds::SCHEDULES, jobs_stopped.clone());
    // Starts at the signal and runs alongside the HTTP drain.
    let mut stopped = jobs_stopped;
    let drain_jobs = async move {
        let _ = stopped.wait_for(|stop| *stop).await;
        let drained = tokio::time::timeout(JOB_DRAIN, async {
            if let Err(err) = runner.await {
                tracing::error!(error = %err, "job runner ended abnormally");
            }
            if let Err(err) = scheduler.await {
                tracing::error!(error = %err, "scheduler ended abnormally");
            }
        })
        .await;
        if drained.is_err() {
            tracing::warn!(
                "jobs still running at exit; their leases expire and another instance resumes them"
            );
        }
    };
    // connect info: rate limit keys on peer IP.
    let serve = axum::serve(
        listener,
        build_app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        // Workers stop claiming now and finish their chunk while requests drain.
        let _ = stop_jobs.send(true);
    });
    let (served, ()) = tokio::join!(std::future::IntoFuture::into_future(serve), drain_jobs);
    served.expect("server");
}

/// Docker waits 10 s after SIGTERM before killing: running chunks get most of it.
const JOB_DRAIN: Duration = Duration::from_secs(8);

// Docker stop sends SIGTERM, terminal sends SIGINT. Either one drains in-flight requests.
// A listener that fails to register waits forever: completing would shut the server down at boot.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "SIGINT listener failed");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::error!(error = %e, "SIGTERM listener failed");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    tracing::info!("shutting down");
}
