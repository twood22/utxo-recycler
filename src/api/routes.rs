use crate::db::{RecycleRepository, RecycleStatus};
use crate::lightning::LnurlClient;
use crate::AppState;
use askama::Template;
use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Form, Json, Router,
};
use qrcode::{render::svg, QrCode};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;

pub fn create_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(index_page))
        .route("/confirm", post(confirm_page))
        .route("/recycle/:id", get(recycle_page))
        .route("/api/recycle", post(create_recycle))
        .route("/api/recycle/:id", get(get_recycle))
        .route("/health", get(health_check))
        .route("/admin/stats", get(admin_stats))
}

// Helper to convert payout_multiplier (1.01) to percent (101)
fn payout_percent(multiplier: f64) -> u32 {
    (multiplier * 100.0).round() as u32
}

// Templates
#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    payout_percent: u32,
}

#[derive(Template)]
#[template(path = "confirm.html")]
struct ConfirmTemplate {
    lightning_address: String,
    cutoff_block_height: u32,
    max_input_sats: u64,
    payout_percent: u32,
    required_confirmations: u32,
}

#[derive(Template)]
#[template(path = "recycle.html")]
struct RecycleTemplate {
    lightning_address: String,
    deposit_address: String,
    qr_code_svg: String,
    status: String,
    status_class: String,
    deposit_txid: Option<String>,
    deposit_amount_sats: Option<u64>,
    deposit_confirmations: u32,
    required_confirmations: u32,
    confirmation_percent: u32,
    deposit_block_height: Option<u32>,
    cutoff_block_height: u32,
    max_input_sats: u64,
    payout_percent: u32,
    is_eligible: bool,
    donation_reason: Option<String>,
    recorded_max_input: Option<u64>,
    payout_amount_sats: Option<u64>,
    payment_preimage: Option<String>,
    is_pending: bool,
}

// API types
#[derive(Deserialize)]
pub struct CreateRecycleRequest {
    pub lightning_address: String,
    pub confirmed: Option<String>,
}

#[derive(Serialize)]
pub struct CreateRecycleResponse {
    pub id: String,
    pub deposit_address: String,
    pub lightning_address: String,
}

#[derive(Serialize)]
pub struct RecycleResponse {
    pub id: String,
    pub lightning_address: String,
    pub deposit_address: String,
    pub status: String,
    pub deposit_txid: Option<String>,
    pub deposit_amount_sats: Option<u64>,
    pub deposit_confirmations: u32,
    pub payout_amount_sats: Option<u64>,
    pub payment_preimage: Option<String>,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// Health check response
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    db: &'static str,
    last_sync: Option<String>,
    last_sync_ago_secs: Option<i64>,
}

// Admin stats response
#[derive(Serialize)]
struct AdminStatsResponse {
    total_recycles: i64,
    by_status: StatusCounts,
    total_deposited_sats: i64,
    total_paid_out_sats: i64,
    total_donations_sats: i64,
    net_sats: i64,
}

#[derive(Serialize)]
struct StatusCounts {
    awaiting_deposit: i64,
    confirming: i64,
    confirmed: i64,
    paid: i64,
    failed: i64,
    donation: i64,
}

#[derive(Deserialize)]
struct AdminQuery {
    token: Option<String>,
}

// Handlers
async fn index_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    HtmlTemplate(IndexTemplate {
        payout_percent: payout_percent(state.config.payout_multiplier),
    })
}

async fn confirm_page(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Form(request): Form<CreateRecycleRequest>,
) -> Response {
    // Rate limiting
    let ip = addr.ip();
    if let Err(retry_after) = state.rate_limiter.check(ip).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("Retry-After", retry_after.to_string())],
            Html(format!(
                "<h1>Rate Limited</h1><p>Too many requests. Please try again in {} seconds.</p><p><a href='/'>Go back</a></p>",
                retry_after
            )),
        )
            .into_response();
    }

    let lightning_address = request.lightning_address.trim().to_lowercase();

    // Validate lightning address format
    if !LnurlClient::validate_lightning_address(&lightning_address) {
        return (
            StatusCode::BAD_REQUEST,
            Html("<h1>Invalid Lightning Address</h1><p>Please enter a valid lightning address (e.g., user@domain.com)</p><p><a href='/'>Go back</a></p>".to_string()),
        )
            .into_response();
    }

    // Validate the lightning address is reachable
    let lnurl_client = LnurlClient::new();
    if let Err(e) = lnurl_client.fetch_pay_params(&lightning_address).await {
        return (
            StatusCode::BAD_REQUEST,
            Html(format!(
                "<h1>Could Not Verify Lightning Address</h1><p>{}</p><p><a href='/'>Go back</a></p>",
                e
            )),
        )
            .into_response();
    }

    // Show confirmation page
    HtmlTemplate(ConfirmTemplate {
        lightning_address,
        cutoff_block_height: state.config.cutoff_block_height,
        max_input_sats: state.config.max_input_sats,
        payout_percent: payout_percent(state.config.payout_multiplier),
        required_confirmations: state.config.required_confirmations,
    })
    .into_response()
}

async fn recycle_page(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let recycle = match RecycleRepository::find_by_id(&state.db, &id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Html("Recycle not found".to_string())).into_response()
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(format!("Error: {}", e)),
            )
                .into_response()
        }
    };

    // Generate QR code
    let qr_code_svg = match QrCode::new(recycle.deposit_address.to_uppercase()) {
        Ok(code) => code
            .render::<svg::Color>()
            .min_dimensions(200, 200)
            .max_dimensions(300, 300)
            .build(),
        Err(_) => String::new(),
    };

    let status_class = match recycle.status {
        RecycleStatus::AwaitingDeposit => "status-awaiting",
        RecycleStatus::Confirming => "status-confirming",
        RecycleStatus::Confirmed => "status-confirmed",
        RecycleStatus::Paid => "status-paid",
        RecycleStatus::Failed => "status-failed",
        RecycleStatus::Donation => "status-donation",
    };

    let is_pending = matches!(
        recycle.status,
        RecycleStatus::AwaitingDeposit | RecycleStatus::Confirming | RecycleStatus::Confirmed
    );

    let confirmation_percent = if state.config.required_confirmations > 0 {
        (recycle.deposit_confirmations * 100 / state.config.required_confirmations).min(100)
    } else {
        100
    };

    let template = RecycleTemplate {
        lightning_address: recycle.lightning_address,
        deposit_address: recycle.deposit_address,
        qr_code_svg,
        status: recycle.status.display_name().to_string(),
        status_class: status_class.to_string(),
        deposit_txid: recycle.deposit_txid,
        deposit_amount_sats: recycle.deposit_amount_sats,
        deposit_confirmations: recycle.deposit_confirmations,
        required_confirmations: state.config.required_confirmations,
        confirmation_percent,
        deposit_block_height: recycle.deposit_block_height,
        cutoff_block_height: state.config.cutoff_block_height,
        max_input_sats: state.config.max_input_sats,
        payout_percent: payout_percent(state.config.payout_multiplier),
        is_eligible: recycle.is_eligible,
        donation_reason: recycle.donation_reason,
        recorded_max_input: recycle.max_input_sats,
        payout_amount_sats: recycle.payout_amount_sats,
        payment_preimage: recycle.payment_preimage,
        is_pending,
    };

    HtmlTemplate(template).into_response()
}

async fn create_recycle(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Form(request): Form<CreateRecycleRequest>,
) -> Response {
    // Rate limiting
    let ip = addr.ip();
    if let Err(retry_after) = state.rate_limiter.check(ip).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("Retry-After", retry_after.to_string())],
            Html(format!(
                "<h1>Rate Limited</h1><p>Too many requests. Please try again in {} seconds.</p><p><a href='/'>Go back</a></p>",
                retry_after
            )),
        )
            .into_response();
    }

    let lightning_address = request.lightning_address.trim().to_lowercase();

    // Ensure user confirmed the eligibility requirements
    if request.confirmed.as_deref() != Some("on") {
        return (
            StatusCode::BAD_REQUEST,
            Html("<h1>Confirmation Required</h1><p>You must confirm that you understand the eligibility requirements.</p><p><a href='/'>Go back</a></p>".to_string()),
        )
            .into_response();
    }

    // Validate lightning address format
    if !LnurlClient::validate_lightning_address(&lightning_address) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid lightning address format. Expected format: user@domain.com"
                    .to_string(),
            }),
        )
            .into_response();
    }

    // Validate the lightning address is reachable
    let lnurl_client = LnurlClient::new();
    if let Err(e) = lnurl_client.fetch_pay_params(&lightning_address).await {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Could not verify lightning address: {}", e),
            }),
        )
            .into_response();
    }

    // Get next address index
    let address_index = match RecycleRepository::increment_address_index(&state.db).await {
        Ok(idx) => idx,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to get address index: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Generate deposit address
    let deposit_address = match state.wallet.get_address(address_index).await {
        Ok(addr) => addr,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to generate deposit address: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Reveal the address in the wallet for monitoring
    if let Err(e) = state.wallet.reveal_addresses_up_to(address_index).await {
        tracing::warn!("Failed to reveal address: {}", e);
    }

    // Create recycle record
    let id = uuid::Uuid::new_v4().to_string();
    match RecycleRepository::create(&state.db, &id, &lightning_address, &deposit_address, address_index).await {
        Ok(_) => {
            // Redirect to the recycle page
            (
                StatusCode::SEE_OTHER,
                [("Location", format!("/recycle/{}", id))],
                "",
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to create recycle: {}", e),
            }),
        )
            .into_response(),
    }
}

async fn get_recycle(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match RecycleRepository::find_by_id(&state.db, &id).await {
        Ok(Some(recycle)) => (
            StatusCode::OK,
            Json(RecycleResponse {
                id: recycle.id,
                lightning_address: recycle.lightning_address,
                deposit_address: recycle.deposit_address,
                status: recycle.status.as_str().to_string(),
                deposit_txid: recycle.deposit_txid,
                deposit_amount_sats: recycle.deposit_amount_sats,
                deposit_confirmations: recycle.deposit_confirmations,
                payout_amount_sats: recycle.payout_amount_sats,
                payment_preimage: recycle.payment_preimage,
            }),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Recycle not found".to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Database error: {}", e),
            }),
        )
            .into_response(),
    }
}

// Health check endpoint
async fn health_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Check database connectivity
    let db_status = match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => "ok",
        Err(_) => "error",
    };

    // Get last sync time
    let last_sync = state.last_sync.read().await;
    let (last_sync_str, last_sync_ago) = match *last_sync {
        Some(dt) => {
            let ago = chrono::Utc::now().signed_duration_since(dt).num_seconds();
            (Some(dt.to_rfc3339()), Some(ago))
        }
        None => (None, None),
    };

    let overall_status = if db_status == "ok" { "ok" } else { "degraded" };

    (
        if overall_status == "ok" {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(HealthResponse {
            status: overall_status,
            db: db_status,
            last_sync: last_sync_str,
            last_sync_ago_secs: last_sync_ago,
        }),
    )
}

// Admin stats endpoint
async fn admin_stats(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AdminQuery>,
) -> Response {
    // Check admin token
    match &state.config.admin_token {
        Some(expected_token) => {
            if query.token.as_ref() != Some(expected_token) {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(ErrorResponse {
                        error: "Invalid or missing admin token".to_string(),
                    }),
                )
                    .into_response();
            }
        }
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Admin endpoint not configured".to_string(),
                }),
            )
                .into_response();
        }
    }

    // Query stats from database
    let stats = match get_admin_stats(&state.db).await {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to get stats: {}", e),
                }),
            )
                .into_response();
        }
    };

    (StatusCode::OK, Json(stats)).into_response()
}

async fn get_admin_stats(db: &sqlx::SqlitePool) -> anyhow::Result<AdminStatsResponse> {
    // Get counts by status
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) as count FROM recycles GROUP BY status"
    )
    .fetch_all(db)
    .await?;

    let mut status_counts = StatusCounts {
        awaiting_deposit: 0,
        confirming: 0,
        confirmed: 0,
        paid: 0,
        failed: 0,
        donation: 0,
    };

    for (status, count) in &counts {
        match status.as_str() {
            "awaiting_deposit" => status_counts.awaiting_deposit = *count,
            "confirming" => status_counts.confirming = *count,
            "confirmed" => status_counts.confirmed = *count,
            "paid" => status_counts.paid = *count,
            "failed" => status_counts.failed = *count,
            "donation" => status_counts.donation = *count,
            _ => {}
        }
    }

    let total_recycles: i64 = counts.iter().map(|(_, c)| c).sum();

    // Get total deposited (from paid recycles)
    let total_deposited: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(deposit_amount_sats), 0) FROM recycles WHERE status = 'paid'"
    )
    .fetch_one(db)
    .await?;

    // Get total paid out
    let total_paid_out: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(payout_amount_sats), 0) FROM recycles WHERE status = 'paid'"
    )
    .fetch_one(db)
    .await?;

    // Get total donations
    let total_donations: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(deposit_amount_sats), 0) FROM recycles WHERE status = 'donation'"
    )
    .fetch_one(db)
    .await?;

    // Net = deposits + donations - payouts (positive = profit)
    let net_sats = total_deposited.0 + total_donations.0 - total_paid_out.0;

    Ok(AdminStatsResponse {
        total_recycles,
        by_status: status_counts,
        total_deposited_sats: total_deposited.0,
        total_paid_out_sats: total_paid_out.0,
        total_donations_sats: total_donations.0,
        net_sats,
    })
}

// Template wrapper for Askama
struct HtmlTemplate<T>(T);

impl<T> IntoResponse for HtmlTemplate<T>
where
    T: Template,
{
    fn into_response(self) -> Response {
        match self.0.render() {
            Ok(html) => Html(html).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render template: {}", err),
            )
                .into_response(),
        }
    }
}
