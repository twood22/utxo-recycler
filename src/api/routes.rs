use crate::db::{RecycleRepository, RecycleStatus};
use crate::lightning::LnurlClient;
use crate::AppState;
use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Form, Json, Router,
};
use qrcode::{render::svg, QrCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn create_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(index_page))
        .route("/recycle/:id", get(recycle_page))
        .route("/api/recycle", post(create_recycle))
        .route("/api/recycle/:id", get(get_recycle))
}

// Templates
#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate;

#[derive(Template)]
#[template(path = "recycle.html")]
struct RecycleTemplate {
    id: String,
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
    payout_amount_sats: Option<u64>,
    payment_preimage: Option<String>,
    is_pending: bool,
}

// API types
#[derive(Deserialize)]
pub struct CreateRecycleRequest {
    pub lightning_address: String,
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

// Handlers
async fn index_page() -> impl IntoResponse {
    HtmlTemplate(IndexTemplate)
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
        id: recycle.id,
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
        payout_amount_sats: recycle.payout_amount_sats,
        payment_preimage: recycle.payment_preimage,
        is_pending,
    };

    HtmlTemplate(template).into_response()
}

async fn create_recycle(
    State(state): State<Arc<AppState>>,
    Form(request): Form<CreateRecycleRequest>,
) -> Response {
    let lightning_address = request.lightning_address.trim().to_lowercase();

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
